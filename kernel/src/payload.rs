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
use core::sync::atomic::{
    AtomicBool, AtomicIsize, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};

use crate::console::FrameBuf;
use crate::mm::{self, AddressSpace};
use crate::{elf, events, frames, hal};

const RE: Ordering = Ordering::Relaxed;

/// Root frame of the kernel-only address space (identity gigapage). The
/// scheduler/idle run under this satp; each payload runs under its own.
static KERNEL_ROOT: AtomicUsize = AtomicUsize::new(0);

/// Build the kernel address space and turn on Sv39 paging. Called once from
/// kmain before traps are armed. After this the kernel runs translated, but
/// the identity gigapage makes every kernel VA equal its physical address, so
/// it is transparent.
pub fn init_paging() {
    let space = AddressSpace::new().expect("kernel root frame");
    mm::map_kernel(&space).expect("kernel gigapage");
    KERNEL_ROOT.store(space.root(), RE);
    hal::write_satp(space.satp());
}

fn kernel_satp() -> u64 {
    AddressSpace::from_root(KERNEL_ROOT.load(RE)).satp()
}

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
static WILD: &[u8] = include_bytes!(env!("PAYLOAD_WILD"));
static WXVIOL: &[u8] = include_bytes!(env!("PAYLOAD_WXVIOL"));
static FORKER: &[u8] = include_bytes!(env!("PAYLOAD_FORKER"));
static LEAKER: &[u8] = include_bytes!(env!("PAYLOAD_LEAKER"));
static DELEGATOR: &[u8] = include_bytes!(env!("PAYLOAD_DELEGATOR"));
static REDELEGATOR: &[u8] = include_bytes!(env!("PAYLOAD_REDELEGATOR"));
static WORKER: &[u8] = include_bytes!(env!("PAYLOAD_WORKER"));
static BADJUMP: &[u8] = include_bytes!(env!("PAYLOAD_BADJUMP"));
// craycast is a C payload (the `cpayloads` cargo feature); the default build is
// pure Rust and needs no C toolchain, so its bytes are empty and its suite —
// the only thing that loads it — is compiled out.
#[cfg(feature = "cpayloads")]
static CRAYCAST: &[u8] = include_bytes!(env!("PAYLOAD_CRAYCAST"));
#[cfg(not(feature = "cpayloads"))]
static CRAYCAST: &[u8] = &[];
// doom is a C payload (the `doom` cargo feature): full doomgeneric DOOM built
// with picolibc. Like craycast its bytes are empty on the default build and its
// suite is compiled out, so a plain `make test` stays pure Rust with no DOOM
// toolchain. Unlike every other image it needs an extra read-only window mapped
// into its address space — the IWAD (see WAD_* and image_window).
#[cfg(feature = "doom")]
static DOOM: &[u8] = include_bytes!(env!("PAYLOAD_DOOM"));
#[cfg(not(feature = "doom"))]
static DOOM: &[u8] = &[];

// ---- DOOM IWAD window (the `doom` feature) ------------------------------
// QEMU loads freedoom1.wad at this physical address — deliberately above
// POOL_END, so the frame allocator never hands it out (see harness/doom.py's
// `-m 256M -device loader,...`). The kernel maps it read-only into the DOOM
// payload's address space at a fixed VA, where the payload's libc file shim
// reads it as the IWAD. Mapping existing physical memory (not pool frames)
// costs only the page-table nodes to reach it; `frames::free` ignores the
// out-of-pool leaves when the space is destroyed.
#[cfg(feature = "doom")]
const WAD_VA: usize = 0x5000_0000;
#[cfg(feature = "doom")]
const WAD_PA: usize = 0x8800_0000; // == POOL_END; the WAD lives just past the pool
#[cfg(feature = "doom")]
const WAD_WINDOW: usize = 32 << 20; // 32 MiB (freedoom1 is ~28.8 MiB)

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
    // wild: reads kernel memory from U-mode → load page fault (isolation).
    Image {
        name: "wild",
        elf: WILD,
        caps: CAP_WRITE,
        deadline: 0,
    },
    // wxviol: writes its own code → store page fault (W^X).
    Image {
        name: "wxviol",
        elf: WXVIOL,
        caps: CAP_WRITE,
        deadline: 0,
    },
    // forker: checkpoints itself; the kernel forks the checkpoint (P8).
    Image {
        name: "forker",
        elf: FORKER,
        caps: CAP_WRITE,
        deadline: 0,
    },
    // leaker: hands the kernel a kernel pointer via write() (confused deputy).
    Image {
        name: "leaker",
        elf: LEAKER,
        caps: CAP_WRITE,
        deadline: 0,
    },
    // M10 delegation chain. Ceilings shrink down the chain so that even a
    // greedy "request everything" at each hop can only attenuate:
    //   delegator {write,spawn,yield} → redelegator {write,spawn} → worker {write}
    Image {
        name: "delegator",
        elf: DELEGATOR,
        caps: CAP_WRITE | CAP_SPAWN | CAP_YIELD,
        deadline: 0,
    },
    Image {
        name: "redelegator",
        elf: REDELEGATOR,
        caps: CAP_WRITE | CAP_SPAWN,
        deadline: 0,
    },
    Image {
        name: "worker",
        elf: WORKER,
        caps: CAP_WRITE,
        deadline: 0,
    },
    // badjump: jumps to an unmapped instruction address → instruction page
    // fault (E1 stimulus: a fetch-side fault distinct from the data faults).
    Image {
        name: "badjump",
        elf: BADJUMP,
        caps: CAP_WRITE,
        deadline: 0,
    },
    // craycast: a C-language fixed-point raycaster (the `cpayloads` feature). Its
    // ELF is empty on the default (pure-Rust) build; only seed_suite_craycast
    // loads it, and that is compiled out unless the feature is on.
    Image {
        name: "craycast",
        elf: CRAYCAST,
        caps: CAP_WRITE,
        deadline: 0,
    },
    // doom: full doomgeneric DOOM (the `doom` feature). Empty ELF on the default
    // build; only seed_suite_doom loads it, and that is compiled out unless the
    // feature is on. No deadline — a full run renders hundreds of frames.
    Image {
        name: "doom",
        elf: DOOM,
        caps: CAP_WRITE,
        deadline: 0,
    },
];

const IMG_HELLO: usize = 0;
const IMG_CRASHER: usize = 1;
const IMG_MUZZLED: usize = 2;
const IMG_SPAWNER: usize = 3;
const IMG_CHILD: usize = 4;
const IMG_RUNAWAY: usize = 5;
const IMG_WILD: usize = 6;
const IMG_WXVIOL: usize = 7;
const IMG_FORKER: usize = 8;
const IMG_LEAKER: usize = 9;
const IMG_DELEGATOR: usize = 10;
const IMG_REDELEGATOR: usize = 11;
const IMG_WORKER: usize = 12;
const IMG_BADJUMP: usize = 13;
#[cfg(feature = "cpayloads")]
const IMG_CRAYCAST: usize = 14;
#[cfg(feature = "doom")]
const IMG_DOOM: usize = 15;

/// A read-only physical window an image needs mapped into its address space on
/// top of its ELF — memory the kernel exposes that the payload does not
/// allocate. Only DOOM uses one: the IWAD QEMU loads above the frame pool.
/// Returns (va, pa, len).
#[cfg(feature = "doom")]
fn image_window(image: usize) -> Option<(usize, usize, usize)> {
    (image == IMG_DOOM).then_some((WAD_VA, WAD_PA, WAD_WINDOW))
}
#[cfg(not(feature = "doom"))]
fn image_window(_image: usize) -> Option<(usize, usize, usize)> {
    None
}

/// Map the read-only user window `[pa, pa+len)` at `va` in `space`, page by
/// page. The window's physical pages lie outside the frame pool, so this maps
/// existing memory (allocating only the page-table nodes to reach it); a bad
/// perms/VA is a structured Err the caller turns into a load fault, never a
/// panic.
fn map_window(space: &AddressSpace, va: usize, pa: usize, len: usize) -> Result<(), &'static str> {
    let mut off = 0;
    while off < len {
        space
            .map_page(va + off, pa + off, mm::R | mm::U)
            .map_err(|_| "window_map_failed")?;
        off += mm::PAGE_SIZE;
    }
    Ok(())
}

/// Map a payload-supplied spawn selector (stable ABI, see payloads/sys) to an
/// image index. Only images a payload is allowed to spawn appear here.
fn spawnable_image(selector: usize) -> Option<usize> {
    match selector {
        0 => Some(IMG_CHILD),       // sys::SPAWNABLE_CHILD
        1 => Some(IMG_REDELEGATOR), // sys::SPAWNABLE_REDELEGATOR
        2 => Some(IMG_WORKER),      // sys::SPAWNABLE_WORKER
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
    /// Root frame of this payload's address space (0 = none / reaped).
    root: AtomicUsize,
    /// Pool frame holding a saved trap frame to resume from (M6 restore);
    /// 0 = start fresh from the ELF entry.
    resume_frame: AtomicUsize,
    /// Event id of this payload's `payload_start` (P12 causal spine): every
    /// event this payload produces — output, exit, fault, kill — names it as
    /// its `caused_by`, so the log is a DAG rooted at the start, not a line.
    start_event: AtomicU64,
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
            root: AtomicUsize::new(0),
            resume_frame: AtomicUsize::new(0),
            start_event: AtomicU64::new(0),
        }
    }
}

static TABLE: [Slot; MAX_PROC] = [const { Slot::new() }; MAX_PROC];
/// pid of the payload currently on the CPU, or NO_PID in kernel/idle.
static CURRENT: AtomicUsize = AtomicUsize::new(NO_PID);

// ---- Checkpoints (M6, P8) -----------------------------------------------

const MAX_SNAP: usize = 4;

/// A frozen point-in-time copy of a payload: an independent deep copy of its
/// address space, its saved register frame, and its CapSet. `restore`/`fork`
/// build a new payload from it.
struct Snapshot {
    valid: AtomicBool,
    root: AtomicUsize,  // deep-copied address space root
    frame: AtomicUsize, // pool frame holding the saved trap frame
    caps: AtomicU32,
    image: AtomicUsize,
}

impl Snapshot {
    const fn new() -> Self {
        Snapshot {
            valid: AtomicBool::new(false),
            root: AtomicUsize::new(0),
            frame: AtomicUsize::new(0),
            caps: AtomicU32::new(0),
            image: AtomicUsize::new(0),
        }
    }
}

static SNAPS: [Snapshot; MAX_SNAP] = [const { Snapshot::new() }; MAX_SNAP];
/// The most recent snapshot id created this suite (1-based; 0 = none).
static LAST_SNAP: AtomicIsize = AtomicIsize::new(0);
/// How many more checkpoint continuations the current suite owes (P8 fork).
static AUTO_FORK: AtomicUsize = AtomicUsize::new(0);

/// The kernel image must not overlap the physical frame pool. Panics loudly
/// at boot if the linker ever lets them collide.
pub fn assert_pool_clear() {
    assert!(
        hal::kernel_end() <= hal::POOL_BASE,
        "kernel image overlaps the frame pool"
    );
}

fn clear_table() {
    for slot in TABLE.iter() {
        slot.state.store(EMPTY, RE);
    }
    free_snapshots();
    AUTO_FORK.store(0, RE);
    LAST_SNAP.store(0, RE);
    #[cfg(feature = "doom")]
    clear_keys();
}

/// Free all snapshots (their deep-copied address spaces + saved frames).
/// Called under the kernel satp when a new suite is seeded.
fn free_snapshots() {
    for s in SNAPS.iter() {
        if s.valid.swap(false, RE) {
            let root = s.root.swap(0, RE);
            if root != 0 {
                AddressSpace::from_root(root).destroy();
            }
            let f = s.frame.swap(0, RE);
            if f != 0 {
                frames::free(f);
            }
        }
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

/// M5 suite (operator-triggered by 'i'): memory isolation. A payload that
/// reaches into kernel memory faults; a payload that writes its own code
/// faults (W^X); a clean payload runs afterward, proving the kernel survived
/// both with its own address space intact.
pub fn seed_suite_m5() {
    clear_table();
    enqueue(IMG_WILD, IMAGES[IMG_WILD].caps, NO_PID);
    enqueue(IMG_WXVIOL, IMAGES[IMG_WXVIOL].caps, NO_PID);
    enqueue(IMG_LEAKER, IMAGES[IMG_LEAKER].caps, NO_PID);
    enqueue(IMG_HELLO, IMAGES[IMG_HELLO].caps, NO_PID);
}

/// M6 suite (operator-triggered by 'f'): checkpoint/restore/fork (P8). The
/// forker checkpoints itself mid-run; after it exits the kernel forks that
/// checkpoint into two independent continuations, each resuming from the
/// checkpoint point (not the top).
pub fn seed_suite_m6() {
    clear_table();
    enqueue(IMG_FORKER, IMAGES[IMG_FORKER].caps, NO_PID);
    AUTO_FORK.store(2, RE); // two what-if continuations
}

/// M10 suite (operator-triggered by 'd'): a two-hop delegation chain (P10). The
/// delegator holds {write,spawn,yield} and spawns a redelegator requesting ALL
/// caps; the grant attenuates to {write,spawn} (the redelegator ceiling). The
/// redelegator, again requesting ALL caps, spawns a worker granted only
/// {write}. Capabilities monotonically shrink down the chain — a greedy request
/// at any hop cannot re-widen it.
pub fn seed_suite_m10() {
    clear_table();
    enqueue(IMG_DELEGATOR, IMAGES[IMG_DELEGATOR].caps, NO_PID);
}

/// E1 stimulus suite (operator-triggered by 'e'): the curated seeded-fault set
/// for the diagnostic-surface evaluation (M12). Four distinct fault modes so
/// the surface benchmark spans more than one kind of bug:
///   crasher → illegal instruction (a privileged CSR write)
///   wild    → load page fault    (reading kernel memory)
///   wxviol  → store page fault   (writing an executable page, W^X)
///   badjump → instruction page fault (fetching from an unmapped address)
/// Kept separate from the milestone suites so the stimulus set can grow
/// (toward the RFC's N≥20) without perturbing any acceptance check.
pub fn seed_suite_eval() {
    clear_table();
    enqueue(IMG_CRASHER, IMAGES[IMG_CRASHER].caps, NO_PID);
    enqueue(IMG_WILD, IMAGES[IMG_WILD].caps, NO_PID);
    enqueue(IMG_WXVIOL, IMAGES[IMG_WXVIOL].caps, NO_PID);
    enqueue(IMG_BADJUMP, IMAGES[IMG_BADJUMP].caps, NO_PID);
}

/// craycast suite (operator 'c', `cpayloads` feature): the C raycaster. It
/// renders a maze in first person and blits each frame via SYS_BLIT — a real,
/// non-Rust workload proving the C toolchain + a framebuffer-as-event surface.
#[cfg(feature = "cpayloads")]
pub fn seed_suite_craycast() {
    clear_table();
    enqueue(IMG_CRAYCAST, IMAGES[IMG_CRAYCAST].caps, NO_PID);
}

/// doom suite (operator 'D', `doom` feature): full doomgeneric DOOM. The kernel
/// maps the IWAD read-only into the payload's address space (image_window); the
/// payload's libc file shim reads the WAD from that window, and its platform
/// layer downscales each rendered frame and blits it via SYS_BLIT — the same
/// framebuffer-as-event surface craycast uses. With no input, DOOM plays its
/// built-in attract-mode demos; under -icount the whole run replays exactly (P9).
#[cfg(feature = "doom")]
pub fn seed_suite_doom() {
    clear_table();
    enqueue(IMG_DOOM, IMAGES[IMG_DOOM].caps, NO_PID);
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
            slot.root.store(0, RE);
            slot.resume_frame.store(0, RE);
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

/// Pick and start the next pending payload; when the queue drains, fork any
/// pending checkpoint continuations (M6), else announce completion and drop
/// to idle. Diverges every path.
///
/// Entered fresh each time (from kmain or via scheduler_resume after a
/// payload leaves). First switches to the kernel address space — so the
/// just-departed payload's page table is no longer active and can be safely
/// freed — then reaps every terminated payload's frames.
pub fn run() -> ! {
    CURRENT.store(NO_PID, RE);
    hal::write_satp(kernel_satp());
    reap_terminated();
    if let Some(pid) = take_next_pending() {
        start(pid);
    }
    // No pending payload: are there checkpoint forks still owed?
    let owed = AUTO_FORK.load(RE);
    let sid = LAST_SNAP.load(RE);
    if owed > 0 && sid > 0 {
        AUTO_FORK.store(owed - 1, RE);
        if let Some(pid) = enqueue_restore((sid - 1) as usize) {
            start(pid);
        }
    }
    emit_suite_done();
    crate::idle()
}

/// Free the address space (and any saved resume frame) of every payload that
/// has reached a terminal state, returning its frames to the pool. Safe to
/// call only under the kernel satp (never while a payload table is active).
fn reap_terminated() {
    for slot in TABLE.iter() {
        if matches!(slot.state.load(RE), EXITED | FAULTED | KILLED) {
            let root = slot.root.swap(0, RE);
            if root != 0 {
                AddressSpace::from_root(root).destroy();
            }
            let rf = slot.resume_frame.swap(0, RE);
            if rf != 0 {
                frames::free(rf);
            }
        }
    }
}

fn start(pid: usize) -> ! {
    // Restored continuation (M6): the address space is already built and a
    // saved trap frame is waiting — resume straight into it.
    let resume = TABLE[pid].resume_frame.load(RE);
    if resume != 0 {
        let root = TABLE[pid].root.load(RE);
        TABLE[pid].state.store(RUNNING, RE);
        TABLE[pid].started_at.store(hal::read_time(), RE);
        CURRENT.store(pid, RE);
        let name = IMAGES[TABLE[pid].image.load(RE)].name;
        emit_start(pid, name, TABLE[pid].caps.load(RE), 0, true);
        hal::resume_user(AddressSpace::from_root(root).satp(), resume);
    }

    let image = TABLE[pid].image.load(RE);
    let img = &IMAGES[image];

    // Build a fresh address space: kernel identity map + the payload's ELF.
    let space = match AddressSpace::new() {
        Some(s) => s,
        None => load_failed(pid, img.name, "out_of_memory"),
    };
    if mm::map_kernel(&space).is_err() {
        space.destroy();
        load_failed(pid, img.name, "kernel_map_failed");
    }
    let entry = match elf::load(img.elf, &space) {
        Ok(entry) => entry,
        Err(e) => {
            space.destroy();
            load_failed(pid, img.name, e.as_str());
        }
    };

    // Some images need an extra read-only window on top of their ELF (DOOM's
    // IWAD). Mapped after the ELF so a collision with a segment surfaces as a
    // load fault, not a silent overwrite.
    if let Some((va, pa, len)) = image_window(image)
        && let Err(reason) = map_window(&space, va, pa, len)
    {
        space.destroy();
        load_failed(pid, img.name, reason);
    }

    // Fresh-start register frame: zeroed GPRs (no kernel value leaks across
    // the privilege boundary), sepc = entry, U-mode. Resume through the same
    // path a restore uses.
    let frame = match frames::alloc() {
        Some(f) => f,
        None => {
            space.destroy();
            load_failed(pid, img.name, "out_of_memory");
        }
    };
    crate::traps::init_frame_to(frame, entry);

    TABLE[pid].root.store(space.root(), RE);
    TABLE[pid].resume_frame.store(frame, RE);
    TABLE[pid].state.store(RUNNING, RE);
    TABLE[pid].started_at.store(hal::read_time(), RE);
    CURRENT.store(pid, RE);
    emit_start(pid, img.name, TABLE[pid].caps.load(RE), entry, false);
    hal::resume_user(space.satp(), frame)
}

fn load_failed(pid: usize, name: &str, reason: &str) -> ! {
    TABLE[pid].state.store(FAULTED, RE);
    emit_load_fault(pid, name, reason);
    run()
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

/// The current payload's address space, if one is running.
fn current_space() -> Option<AddressSpace> {
    let pid = current_pid()?;
    let root = TABLE[pid].root.load(RE);
    (root != 0).then(|| AddressSpace::from_root(root))
}

/// Page-table walk of `va` in the current payload's space (for the P6 fault
/// frame). None if no payload is running.
pub fn current_pagewalk(va: usize) -> Option<mm::WalkChain> {
    Some(current_space()?.walk(va))
}

/// The `payload_start` event id of the running payload (P12 causal parent for
/// its fault frame). None if no payload is running (a kernel fault has no
/// payload cause).
pub fn current_cause() -> Option<u64> {
    Some(TABLE[current_pid()?].start_event.load(RE))
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

/// Checkpoint the calling payload (P8). Deep-copies its address space, saves
/// its register frame (with a0 forced to 0 so a restored continuation can
/// tell it apart from the original), records its CapSet, and returns a
/// positive snapshot id. `frame` is the payload's trap frame at the
/// `sys_snapshot` ecall (sepc already advanced past it).
pub fn on_snapshot(frame: &crate::traps::TrapFrame) -> isize {
    let pid = match current_pid() {
        Some(p) => p,
        None => return crate::syscall::EINVAL,
    };
    let space = match current_space() {
        Some(s) => s,
        None => return crate::syscall::EINVAL,
    };
    let sid = match SNAPS.iter().position(|s| !s.valid.load(RE)) {
        Some(i) => i,
        None => return crate::syscall::EAGAIN,
    };
    let snap_as = match space.deep_copy() {
        Some(a) => a,
        None => return crate::syscall::ENOMEM,
    };
    let snap_frame = match frames::alloc() {
        Some(f) => f,
        None => {
            snap_as.destroy();
            return crate::syscall::ENOMEM;
        }
    };
    crate::traps::save_frame_to(frame, snap_frame, 0); // restores see a0 = 0
    SNAPS[sid].root.store(snap_as.root(), RE);
    SNAPS[sid].frame.store(snap_frame, RE);
    SNAPS[sid].caps.store(TABLE[pid].caps.load(RE), RE);
    SNAPS[sid].image.store(TABLE[pid].image.load(RE), RE);
    SNAPS[sid].valid.store(true, RE);
    LAST_SNAP.store(sid as isize + 1, RE);
    emit_snapshot(pid, sid + 1);
    (sid + 1) as isize
}

/// Build a new pending payload that resumes from snapshot `sid` (restore /
/// fork). Deep-copies the snapshot's address space (so continuations are
/// independent) and its saved frame. Returns the new pid, or None.
fn enqueue_restore(sid: usize) -> Option<usize> {
    if !SNAPS[sid].valid.load(RE) {
        return None;
    }
    let new_as = AddressSpace::from_root(SNAPS[sid].root.load(RE)).deep_copy()?;
    let new_frame = match frames::alloc() {
        Some(f) => f,
        None => {
            new_as.destroy();
            return None;
        }
    };
    let mut buf = [0u8; mm::PAGE_SIZE];
    if !hal::phys_read(SNAPS[sid].frame.load(RE), &mut buf) || !hal::phys_write(new_frame, &buf) {
        new_as.destroy();
        frames::free(new_frame);
        return None;
    }
    for (pid, slot) in TABLE.iter().enumerate() {
        if slot.state.load(RE) == EMPTY {
            let image = SNAPS[sid].image.load(RE);
            slot.image.store(image, RE);
            // Re-clamp to the image ceiling on restore too (defense in depth,
            // per the M10 attenuation audit): this is the one caps writer that
            // copies rather than derives, so make the ⊆ ceiling invariant
            // explicit and self-defending instead of relying on the snapshot
            // having been clamped upstream.
            slot.caps
                .store(SNAPS[sid].caps.load(RE) & IMAGES[image].caps, RE);
            slot.parent.store(NO_PID, RE);
            slot.exit_code.store(0, RE);
            slot.deadline.store(0, RE);
            slot.started_at.store(0, RE);
            slot.root.store(new_as.root(), RE);
            slot.resume_frame.store(new_frame, RE);
            slot.state.store(PENDING, RE);
            return Some(pid);
        }
    }
    new_as.destroy();
    frames::free(new_frame);
    None
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
    // Read the payload's bytes through its own page table — a bad pointer is
    // rejected here (EFAULT), never a kernel access.
    let space = current_space().ok_or(())?;
    if !space.copy_from_user(ptr, &mut buf[..n]) {
        return Err(());
    }
    let pid = current_pid().unwrap_or(NO_PID);
    let caused_by = TABLE.get(pid).map_or(0, |s| s.start_event.load(RE));
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        // "untrusted":true and the bytes confined to a JSON string are the
        // P7 provenance seam: payload output can never be read by the
        // operator as kernel-issued instructions.
        let _ = write!(
            f,
            r#"{{"id":{},"type":"payload_output","pid":{pid},"untrusted":true,"caused_by":{caused_by},"len":{n},"data":""#,
            events::next_id()
        );
        let _ = f.write_json_escaped_bytes(&buf[..n]);
        let _ = f.write_str(r#""}"#);
        f.emit();
    });
    Ok(())
}

// ---- blit (SYS_BLIT): the framebuffer as a deterministic event -----------

/// Shade ramp: framebuffer byte (0..255) → an ASCII glyph. All glyphs are
/// JSON-safe (no `"`/`\`/control), so rows go straight into a JSON string.
const RAMP: [&str; 10] = [" ", ".", ":", "-", "=", "+", "*", "#", "%", "@"];
/// Output caps chosen so even the largest accepted ASCII frame fits one
/// FrameBuf (2 KiB): 72×24 → ~1.8 KiB with the header. A payload asking for
/// more is refused (EINVAL), never silently dropped, and `overflowed()` is a
/// second backstop.
const BLIT_MAXW: usize = 72;
const BLIT_MAXH: usize = 24;

/// `blit(ptr, w, h)`: a payload hands the kernel a `w×h` byte framebuffer in
/// its own address space; the kernel reads it **through the payload's page
/// table** (a bad pointer is EFAULT, never a kernel read — the same
/// confused-deputy defense as `write`), maps each byte to an ASCII glyph, and
/// emits a `frame` event with an FNV checksum of the raw bytes. Under -icount
/// the same run yields byte-identical frames + checksums (P9). Requires
/// `Cap::Write`. This is the "display as an event/resource" seam a doomgeneric
/// port would render into.
pub fn on_blit(ptr: usize, w: usize, h: usize) -> isize {
    if !current_has(Cap::Write) {
        emit_denied("blit", "write");
        return crate::syscall::ENOCAP;
    }
    if w == 0 || h == 0 || w > BLIT_MAXW || h > BLIT_MAXH {
        return crate::syscall::EINVAL;
    }
    let space = match current_space() {
        Some(s) => s,
        None => return crate::syscall::EINVAL,
    };
    let pid = current_pid().unwrap_or(NO_PID);
    let mut ok = true;
    let mut overflow = false;
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"frame","pid":{pid},"untrusted":true,"w":{w},"h":{h},"rows":["#,
            events::next_id()
        );
        let mut checksum: u64 = 0xcbf2_9ce4_8422_2325;
        let mut row = [0u8; BLIT_MAXW];
        for y in 0..h {
            if y > 0 {
                let _ = f.write_str(",");
            }
            let _ = f.write_str("\"");
            if !space.copy_from_user(ptr.wrapping_add(y * w), &mut row[..w]) {
                ok = false;
                break;
            }
            for &v in &row[..w] {
                checksum = (checksum ^ v as u64).wrapping_mul(0x0000_0100_0000_01b3);
                let _ = f.write_str(RAMP[(v as usize * (RAMP.len() - 1)) / 255]);
            }
            let _ = f.write_str("\"");
        }
        if ok {
            let _ = write!(f, r#"],"checksum":"0x{checksum:x}"}}"#);
            if f.overflowed() {
                overflow = true;
            } else {
                f.emit();
            }
        }
    });
    if !ok {
        crate::syscall::EFAULT
    } else if overflow {
        crate::syscall::EINVAL
    } else {
        0
    }
}

// ---- frame dump (SYS_FRAME): full-color keyframes as base64 chunks --------

/// `frame_rgb(ptr, len, seq)`: stream one chunk of the payload's raw RGB
/// framebuffer out as an `fbchunk` event. Where `blit` emits a tiny ASCII
/// thumbnail as the deterministic in-band surface, this ships the true pixels
/// (DOOM's full 320×200 color screen, in `len`-byte pieces the host stitches
/// back together and saves as a PNG) — a richer "display as a resource" seam.
/// Reads the bytes **through the payload's page table** (a bad pointer is
/// EFAULT, never a kernel read — same confused-deputy defense as write/blit),
/// base64-encodes them (`+`/`/`/`=` need no JSON escaping) and tags the event
/// untrusted (P7). Requires `Cap::Write`. `doom` feature only.
#[cfg(feature = "doom")]
pub fn on_frame_rgb(ptr: usize, len: usize, seq: usize) -> isize {
    if !current_has(Cap::Write) {
        emit_denied("frame", "write");
        return crate::syscall::ENOCAP;
    }
    // 3-aligned so base64 never pads mid-stream; 1440 B → 1920 base64 chars,
    // which with the header fits one 2 KiB FrameBuf.
    const FB_CHUNK: usize = 1440;
    if len == 0 || len > FB_CHUNK {
        return crate::syscall::EINVAL;
    }
    let space = match current_space() {
        Some(s) => s,
        None => return crate::syscall::EINVAL,
    };
    let mut buf = [0u8; FB_CHUNK];
    if !space.copy_from_user(ptr, &mut buf[..len]) {
        return crate::syscall::EFAULT;
    }
    let pid = current_pid().unwrap_or(NO_PID);
    let mut overflow = false;
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"fbchunk","pid":{pid},"untrusted":true,"seq":{seq},"n":{len},"data":""#,
            events::next_id()
        );
        write_base64(&mut f, &buf[..len]);
        let _ = f.write_str(r#""}"#);
        if f.overflowed() {
            overflow = true;
        } else {
            f.emit();
        }
    });
    if overflow { crate::syscall::EINVAL } else { 0 }
}

/// Standard-alphabet base64 into a FrameBuf. The alphabet is JSON-string-safe,
/// so the output drops straight into an event with no escaping.
#[cfg(feature = "doom")]
fn write_base64(f: &mut FrameBuf, bytes: &[u8]) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let g = |i: u32| ALPHABET[(i & 63) as usize];
    let (chunks, rem) = bytes.as_chunks::<3>();
    for c in chunks {
        let n = (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
        let s = [g(n >> 18), g(n >> 12), g(n >> 6), g(n)];
        let _ = f.write_str(core::str::from_utf8(&s).unwrap_or("?"));
    }
    if !rem.is_empty() {
        let b1 = if rem.len() > 1 { rem[1] as u32 } else { 0 };
        let n = (rem[0] as u32) << 16 | b1 << 8;
        let s = if rem.len() == 1 {
            [g(n >> 18), g(n >> 12), b'=', b'=']
        } else {
            [g(n >> 18), g(n >> 12), g(n >> 6), b'=']
        };
        let _ = f.write_str(core::str::from_utf8(&s).unwrap_or("?"));
    }
}

// ---- operator input (SYS_GETKEY, `doom` feature): keys as a queue ---------
// The agentic input seam: while a payload runs, serial bytes are drained (by
// the timer tick) into a small ring the payload pops via SYS_GETKEY — the
// operator/agent acts on the workload *through the kernel*, one byte per key
// event (low 7 bits = symbol, bit 7 = release; the payload owns the symbol →
// key mapping). Byte 0x03 is never queued: it is the operator kill (P1/P2 —
// a live remediation, the E2 seed). Interactive input is host-timed, so a
// keyed run is replayed via QEMU record/replay (M7) rather than by rebooting.

#[cfg(feature = "doom")]
const KEY_RING_SIZE: usize = 64;
#[cfg(feature = "doom")]
static KEY_RING: [AtomicU8; KEY_RING_SIZE] = [const { AtomicU8::new(0) }; KEY_RING_SIZE];
#[cfg(feature = "doom")]
static KEY_HEAD: AtomicUsize = AtomicUsize::new(0); // next slot to pop
#[cfg(feature = "doom")]
static KEY_TAIL: AtomicUsize = AtomicUsize::new(0); // next slot to push
/// The operator kill byte (ETX / Ctrl-C): kill the running payload, never
/// queued as input.
#[cfg(feature = "doom")]
const KEY_OP_KILL: u8 = 0x03;

/// Drain pending serial bytes into the key ring. Called from the timer tick
/// (trap context, interrupts hardware-off) — but only while a payload is
/// running, so the idle command loop / MCP reader never lose bytes to it.
/// Returns true if the operator killed the current payload (0x03), in which
/// case it is already marked KILLED and the caller must redirect to the
/// scheduler.
#[cfg(feature = "doom")]
pub fn drain_keys() -> bool {
    let pid = match current_pid() {
        Some(p) => p,
        None => return false,
    };
    if TABLE[pid].state.load(RE) != RUNNING {
        return false;
    }
    let mut killed = false;
    while let Some(b) = hal::console_getchar() {
        if b == KEY_OP_KILL {
            TABLE[pid].state.store(KILLED, RE);
            emit_killed_operator(pid);
            killed = true;
            // Keep draining: flush any queued bytes behind the kill.
            continue;
        }
        let tail = KEY_TAIL.load(RE);
        if tail.wrapping_sub(KEY_HEAD.load(RE)) >= KEY_RING_SIZE {
            continue; // ring full: drop newest (a lost key beats a stale queue)
        }
        KEY_RING[tail % KEY_RING_SIZE].store(b, RE);
        KEY_TAIL.store(tail.wrapping_add(1), RE);
    }
    killed
}

/// `getkey()`: pop one key byte for the running payload; EAGAIN when the ring
/// is empty. No capability required: it reads only the input queue the
/// operator explicitly fed to this (single, running) payload — input is a
/// grant by construction, unlike output (write/blit) which exfiltrates.
#[cfg(feature = "doom")]
pub fn on_getkey() -> isize {
    let head = KEY_HEAD.load(RE);
    if head == KEY_TAIL.load(RE) {
        return crate::syscall::EAGAIN;
    }
    let b = KEY_RING[head % KEY_RING_SIZE].load(RE);
    KEY_HEAD.store(head.wrapping_add(1), RE);
    b as isize
}

/// Empty the key ring (on suite seed, so stale input never leaks into a new
/// payload's queue).
#[cfg(feature = "doom")]
fn clear_keys() {
    KEY_HEAD.store(0, RE);
    KEY_TAIL.store(0, RE);
}

/// The operator killed the payload mid-run (P1: remediation is a structured
/// event, not a power cycle). Shape mirrors the deadline kill.
#[cfg(feature = "doom")]
fn emit_killed_operator(pid: usize) {
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"payload_killed","pid":{pid},"reason":"operator","caused_by":{}}}"#,
            events::next_id(),
            TABLE[pid].start_event.load(RE),
        );
        f.emit();
    });
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

fn state_name(s: u8) -> &'static str {
    match s {
        EMPTY => "empty",
        PENDING => "pending",
        RUNNING => "running",
        EXITED => "exited",
        FAULTED => "faulted",
        KILLED => "killed",
        _ => "unknown", // never stored; a false "empty" would mislead an operator
    }
}

// The `processes` resource must fit one 2 KiB frame (else `emit_rpc` falls back
// to a "response too large" error instead of the table). Worst case per slot is
// ~110 bytes (pid + longest name + state + a 20-char signed exit_code + the full
// caps array); this guard fails the BUILD if MAX_PROC ever grows past what a
// frame holds, rather than silently truncating at runtime.
const _: () = assert!(MAX_PROC * 128 + 128 < FrameBuf::CAPACITY);

/// The process table as an MCP resource body (`processes`): every non-empty
/// slot with its pid, image name, state, exit code, and CapSet. This is the
/// P4 `/payloads` resource — the operator's view of what has run and how it
/// ended, structured, not a printf dump.
pub fn write_process_table(f: &mut FrameBuf) -> core::fmt::Result {
    f.write_str(r#"{"processes":["#)?;
    let mut first = true;
    for (pid, slot) in TABLE.iter().enumerate() {
        let st = slot.state.load(RE);
        if st == EMPTY {
            continue;
        }
        if !first {
            f.write_str(",")?;
        }
        first = false;
        let name = IMAGES
            .get(slot.image.load(RE))
            .map(|i| i.name)
            .unwrap_or("?");
        write!(
            f,
            r#"{{"pid":{pid},"name":"{name}","state":"{}","exit_code":{},"caps":"#,
            state_name(st),
            slot.exit_code.load(RE),
        )?;
        caps_json(f, slot.caps.load(RE));
        f.write_str("}")?;
    }
    f.write_str("]}")
}

/// The self-describing surface (P5) as the MCP `spec` resource: the syscall
/// table, the capability lattice, and the memory map — the contract an agent
/// discovers instead of being handed a SPEC.md that drifts. Static: it is the
/// kernel describing its own ABI.
pub fn write_spec(f: &mut FrameBuf) -> core::fmt::Result {
    use crate::syscall::{SYS_BLIT, SYS_EXIT, SYS_SNAPSHOT, SYS_SPAWN, SYS_WRITE, SYS_YIELD};
    f.write_str(r#"{"proto":0,"arch":"riscv64","syscalls":["#)?;
    let syscalls: [(u64, &str); 6] = [
        (SYS_EXIT, "exit"),
        (SYS_WRITE, "write"),
        (SYS_YIELD, "yield"),
        (SYS_SPAWN, "spawn"),
        (SYS_SNAPSHOT, "snapshot"),
        (SYS_BLIT, "blit"),
    ];
    for (i, (num, name)) in syscalls.iter().enumerate() {
        if i > 0 {
            f.write_str(",")?;
        }
        write!(f, r#"{{"num":{num},"name":"{name}"}}"#)?;
    }
    // The doom build's ABI extensions must self-describe too (P5): an agent
    // discovers frame/getkey from the spec, not from a doc that drifts.
    #[cfg(feature = "doom")]
    {
        use crate::syscall::{SYS_FRAME, SYS_GETKEY};
        write!(
            f,
            r#",{{"num":{SYS_FRAME},"name":"frame"}},{{"num":{SYS_GETKEY},"name":"getkey"}}"#
        )?;
    }
    f.write_str(r#"],"caps":["#)?;
    let caps: [(&str, u32); 3] = [
        ("write", CAP_WRITE),
        ("yield", CAP_YIELD),
        ("spawn", CAP_SPAWN),
    ];
    for (i, (name, bit)) in caps.iter().enumerate() {
        if i > 0 {
            f.write_str(",")?;
        }
        write!(f, r#"{{"name":"{name}","bit":{bit}}}"#)?;
    }
    write!(
        f,
        r#"],"memory":{{"kernel_base":"0x80200000","pool_base":"0x{:x}","pool_end":"0x{:x}"}}"#,
        hal::POOL_BASE,
        hal::POOL_END,
    )?;
    // The IWAD window (doom build): the one kernel-provided mapping a payload
    // gets beyond its ELF — an agent reading the spec learns it exists.
    #[cfg(feature = "doom")]
    write!(
        f,
        r#","windows":[{{"name":"iwad","image":"doom","va":"0x{WAD_VA:x}","len":{WAD_WINDOW},"perms":"r"}}]"#
    )?;
    f.write_str("}")
}

fn emit_start(pid: usize, name: &str, caps: u32, entry: usize, restored: bool) {
    hal::without_interrupts(|| {
        // Record this start as the causal root of everything this payload does
        // (P12). A spawned child names its parent's start as *its* cause, so a
        // delegation chain is a real path in the DAG; a top-level payload has
        // no in-band cause event, so `caused_by` is null (the operator).
        let ev = events::next_id();
        TABLE[pid].start_event.store(ev, RE);
        let parent = TABLE[pid].parent.load(RE);
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{ev},"type":"payload_start","pid":{pid},"name":"{name}","entry":"0x{entry:x}","restored":{restored},"caused_by":"#,
        );
        if parent != NO_PID {
            let _ = write!(f, "{}", TABLE[parent].start_event.load(RE));
        } else {
            let _ = f.write_str("null");
        }
        let _ = f.write_str(r#","caps":"#);
        caps_json(&mut f, caps);
        let _ = f.write_str("}");
        f.emit();
    });
}

fn emit_snapshot(pid: usize, id: usize) {
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"snapshot","pid":{pid},"snapshot":{id}}}"#,
            events::next_id()
        );
        f.emit();
    });
}

fn emit_exit(pid: usize, code: isize) {
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"payload_exit","pid":{pid},"code":{code},"caused_by":{}}}"#,
            events::next_id(),
            TABLE[pid].start_event.load(RE),
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
            r#"{{"id":{},"type":"payload_killed","pid":{pid},"reason":"deadline","elapsed":{elapsed},"deadline":{deadline},"caused_by":{}}}"#,
            events::next_id(),
            TABLE[pid].start_event.load(RE),
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
