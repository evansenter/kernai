#![no_std]
#![no_main]
#![deny(unsafe_code)]
// ^ deny, not forbid, at the crate root only: `forbid` here could not be
// re-allowed for the hal island below. Every other module carries
// #![forbid(unsafe_code)] itself (enforced by ci/unsafe_budget.sh).

mod console;
mod events;
#[allow(unsafe_code)]
mod hal;
mod traps;

use core::fmt::Write;

use console::FrameBuf;

/// Kernel proper. Called exactly once from hal::boot with a stack and
/// zeroed .bss. `dtb` is parked until something needs the device tree.
///
/// After boot the kernel free-runs timer ticks and serves single-byte
/// serial commands — the embryo of P1's externalized control plane:
///   'r' → dump the trap ring (P11 seed)
///   'x' → deliberately execute an illegal instruction (M2 acceptance 3)
pub fn kmain(_hartid: usize, _dtb: usize) -> ! {
    let mut f = FrameBuf::new();
    let _ = write!(
        f,
        r#"{{"id":{},"type":"hello","name":"kernai","proto":0}}"#,
        events::next_id()
    );
    f.emit();

    hal::traps_init();
    traps::arm_first_tick();
    hal::enable_timer_interrupts();

    loop {
        match hal::console_getchar() {
            Some(b'r') => traps::emit_ring_dump(),
            Some(b'x') => hal::trigger_illegal_instruction(),
            _ => hal::wait_for_interrupt(), // park until the next tick
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use core::sync::atomic::{AtomicBool, Ordering};

    // A panic inside this handler would recurse until the stack silently
    // overwrote .bss (no guard page yet) — second entry goes straight down.
    static IN_PANIC: AtomicBool = AtomicBool::new(false);
    if IN_PANIC.swap(true, Ordering::Relaxed) {
        hal::shutdown(true);
    }

    // A panic is a kernel bug, but it still reports structure, not silence:
    // this frame is the earliest ancestor of the P6 diagnostic frame.
    // Interrupts off for the whole build+emit — same no-interleaving
    // guarantee every other emission path has (a tick mid-frame would
    // corrupt the wire exactly when the report matters most).
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"panic","location":""#,
            events::next_id()
        );
        if let Some(loc) = info.location() {
            let mut esc = FmtCapture(&mut f);
            let _ = write!(esc, "{}:{}", loc.file(), loc.line());
        }
        let _ = f.write_str(r#"","msg":""#);
        let mut msg = FmtCapture(&mut f);
        let _ = write!(msg, "{}", info.message());
        let _ = f.write_str(r#""}"#);
        f.emit();
    });
    hal::shutdown(true)
}

/// Routes a format stream through FrameBuf's JSON escaping.
struct FmtCapture<'a>(&'a mut FrameBuf);

impl Write for FmtCapture<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.write_json_escaped(s)
    }
}
