#![no_std]
#![no_main]
//! E1 stimulus: stack overflow. Unbounded recursion with a real frame per
//! call marches sp below the mapped stack until a store faults (scause 15).
//! Same cause class as `wxviol` (store page fault), but the root cause reads
//! completely differently from the structured frame: stval sits just under
//! the stack region and — the giveaway — the saved `sp` register is within a
//! page of stval. The classic line shows a bare store_page_fault.

#[inline(never)]
fn recurse(n: u64) -> u64 {
    // A stack-resident buffer keeps every frame fat and un-optimizable.
    let frame = core::hint::black_box([n; 32]);
    if n == 0 {
        frame[0]
    } else {
        recurse(n - 1).wrapping_add(core::hint::black_box(frame[31]))
    }
}

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"recursing until the stack runs out");
    let v = recurse(u64::MAX); // faults long before this returns
    sys::write(if v == 0 {
        b"impossible"
    } else {
        b"unreachable"
    });
}
