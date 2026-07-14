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

use core::fmt::Write;

use console::FrameBuf;

/// Kernel proper. Called exactly once from hal::boot with a stack and
/// zeroed .bss. `dtb` is parked until something needs the device tree.
pub fn kmain(_hartid: usize, _dtb: usize) -> ! {
    let mut f = FrameBuf::new();
    let _ = write!(
        f,
        r#"{{"id":{},"type":"hello","name":"kernai","proto":0,"milestone":"M1"}}"#,
        events::next_id()
    );
    f.emit();

    hal::shutdown(false)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // A panic is a kernel bug, but it still reports structure, not silence:
    // this frame is the earliest ancestor of the P6 diagnostic frame.
    let mut f = FrameBuf::new();
    let _ = write!(
        f,
        r#"{{"id":{},"type":"panic","location":""#,
        events::next_id()
    );
    if let Some(loc) = info.location() {
        let _ = write!(f, "{}:{}", loc.file(), loc.line());
    }
    let _ = f.write_str(r#"","msg":""#);
    let mut msg = FmtCapture(&mut f);
    let _ = write!(msg, "{}", info.message());
    let _ = f.write_str(r#""}"#);
    f.emit();
    hal::shutdown(true)
}

/// Routes a format stream through FrameBuf's JSON escaping.
struct FmtCapture<'a>(&'a mut FrameBuf);

impl Write for FmtCapture<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.write_json_escaped(s)
    }
}
