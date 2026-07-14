#![no_std]
//! Minimal userspace runtime for kernai acceptance payloads: `_start`,
//! stack, ecall shims, panic → exit(101).
//!
//! Payloads are U-mode test fixtures, not kernel code — the kernel/src/hal
//! unsafe budget does not apply out here (DECISIONS.md). Keep the unsafe
//! minimal anyway: the entry asm and one ecall shim.
//!
//! Syscall ABI v0: number in a7, args in a0..a2, return in a0
//! (0 = ok, negative = error; see kernel/src/syscall.rs).

use core::arch::{asm, global_asm};

global_asm!(
    r#"
    .section .text.entry
    .globl _start
_start:
    // The kernel's loader has zeroed .bss (it zero-fills memsz past filesz).
    la   sp, __stack_top
    call pmain
    li   a0, 0
    li   a7, 0          // SYS_EXIT: pmain returning means success
    ecall
1:  j    1b
    "#
);

pub const SYS_EXIT: usize = 0;
pub const SYS_WRITE: usize = 1;
pub const SYS_YIELD: usize = 2;
pub const SYS_SPAWN: usize = 3;
pub const SYS_SNAPSHOT: usize = 4;

// Capability bits (must match kernel/src/payload.rs).
pub const CAP_WRITE: usize = 1 << 0;
pub const CAP_YIELD: usize = 1 << 1;
pub const CAP_SPAWN: usize = 1 << 2;

// Spawnable image selectors (stable ABI; kernel maps these to images).
pub const SPAWNABLE_CHILD: usize = 0;
pub const SPAWNABLE_REDELEGATOR: usize = 1;
pub const SPAWNABLE_WORKER: usize = 2;

pub fn syscall(nr: usize, a0: usize, a1: usize, a2: usize) -> isize {
    let ret: isize;
    // SAFETY: `ecall` transfers to the kernel's trap handler, which returns
    // a result in a0 and preserves the other registers we declare.
    unsafe {
        asm!(
            "ecall",
            in("a7") nr,
            inlateout("a0") a0 => ret,
            in("a1") a1,
            in("a2") a2,
        );
    }
    ret
}

pub fn exit(code: usize) -> ! {
    syscall(SYS_EXIT, code, 0, 0);
    // The kernel never resumes an exited payload; satisfy the type.
    #[allow(clippy::empty_loop)]
    loop {}
}

pub fn write(bytes: &[u8]) -> isize {
    syscall(SYS_WRITE, bytes.as_ptr() as usize, bytes.len(), 0)
}

/// write() with a raw pointer/length — used by the isolation fixture to hand
/// the kernel a pointer it shouldn't follow (confused-deputy test).
pub fn write_raw(ptr: usize, len: usize) -> isize {
    syscall(SYS_WRITE, ptr, len, 0)
}

pub fn yield_now() -> isize {
    syscall(SYS_YIELD, 0, 0, 0)
}

/// Spawn a sub-payload (`image` selector) requesting `caps`. The kernel
/// attenuates the grant to a subset of our own capabilities (P10). Returns
/// the child pid, or a negative errno.
pub fn spawn(image: usize, caps: usize) -> isize {
    syscall(SYS_SPAWN, image, caps, 0)
}

/// Checkpoint the calling payload (P8). Returns a positive snapshot id in the
/// original run, and 0 in each restored/forked continuation — the fork()-style
/// distinction that lets a what-if continuation know which branch it is.
/// Negative on error.
pub fn snapshot() -> isize {
    syscall(SYS_SNAPSHOT, 0, 0, 0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(101)
}
