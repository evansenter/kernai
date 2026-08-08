#![no_std]
#![no_main]
//! E1 stimulus: a misaligned AMO — `amoadd.w` on a 2-byte-offset address.
//! Ordinary rv64 loads/stores tolerate misalignment (hardware fixes them up),
//! but atomics never do (QEMU reports the AMO's read phase: scause 4,
//! load-address-misaligned). The root
//! cause (address % 4 != 0, and WHICH register held the bad pointer) is only
//! visible with the register file + stval together.

#[repr(align(4))]
struct Aligned([u8; 16]);
static mut BUF: Aligned = Aligned([0; 16]);

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"atomic add on a misaligned address");
    // SAFETY: deliberately trapping — an AMO on a non-4-aligned address
    // raises a misaligned-address exception. That trap is the test.
    unsafe {
        let p = (&raw mut BUF.0).cast::<u8>().add(2); // 4-aligned base + 2
        core::arch::asm!(
            "amoadd.w {tmp}, {tmp}, ({addr})",
            tmp = inout(reg) 1u64 => _,
            addr = in(reg) p,
        );
    }
}
