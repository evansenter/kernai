#![forbid(unsafe_code)]
//! Frame emission over the SBI console. Mirrors harness/framing.py exactly:
//! 0xAA 0x99 magic, u32 LE length, UTF-8 JSON payload. Dumb on purpose.

use core::fmt::{self, Write};

use crate::hal;

const MAGIC: [u8; 2] = [0xAA, 0x99];

/// Fixed-capacity payload buffer. Build the JSON with `write!`, then `emit()`.
/// Overflow poisons the frame (it will not emit) rather than truncating:
/// a silently truncated diagnostic is worse than a loudly missing one.
pub struct FrameBuf {
    buf: [u8; Self::CAPACITY],
    len: usize,
    overflow: bool,
}

impl FrameBuf {
    pub const CAPACITY: usize = 1024;

    pub const fn new() -> Self {
        FrameBuf {
            buf: [0; Self::CAPACITY],
            len: 0,
            overflow: false,
        }
    }

    /// Write `s` as JSON string *content*: escapes `"`, `\` and control
    /// characters. Quotes are the caller's job.
    pub fn write_json_escaped(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            match c {
                '"' => self.write_str("\\\"")?,
                '\\' => self.write_str("\\\\")?,
                '\n' => self.write_str("\\n")?,
                '\r' => self.write_str("\\r")?,
                '\t' => self.write_str("\\t")?,
                c if (c as u32) < 0x20 => write!(self, "\\u{:04x}", c as u32)?,
                c => self.write_char(c)?,
            }
        }
        Ok(())
    }

    /// Send the frame over the serial link. A poisoned (overflowed) buffer
    /// emits nothing — the absence is the signal.
    pub fn emit(&self) {
        if self.overflow {
            return;
        }
        for b in MAGIC {
            hal::console_putchar(b);
        }
        for b in (self.len as u32).to_le_bytes() {
            hal::console_putchar(b);
        }
        for &b in &self.buf[..self.len] {
            hal::console_putchar(b);
        }
    }
}

impl Write for FrameBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        if self.len + bytes.len() > Self::CAPACITY {
            self.overflow = true;
            return Err(fmt::Error);
        }
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}
