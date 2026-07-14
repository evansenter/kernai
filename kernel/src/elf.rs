#![forbid(unsafe_code)]
//! Minimal ELF64 loader for rv64 payloads. Parses a byte slice and copies
//! PT_LOAD segments into the payload arena via the bounds-checked
//! `hal::arena_*` helpers — so loading untrusted-ish payload images stays
//! entirely in safe code. No dynamic linking, no relocations (payloads are
//! statically linked at ARENA_BASE); those are hard non-goals.

use crate::hal;

/// Why an image could not be loaded. These become fields in the load-fault
/// diagnostic frame (P6) rather than panics.
#[derive(Clone, Copy)]
pub enum ElfError {
    TooShort,
    BadMagic,
    NotElf64Rv,
    BadProgramHeaders,
    SegmentOutOfArena,
    EntryOutOfArena,
}

impl ElfError {
    pub fn as_str(self) -> &'static str {
        match self {
            ElfError::TooShort => "too_short",
            ElfError::BadMagic => "bad_magic",
            ElfError::NotElf64Rv => "not_elf64_riscv",
            ElfError::BadProgramHeaders => "bad_program_headers",
            ElfError::SegmentOutOfArena => "segment_out_of_arena",
            ElfError::EntryOutOfArena => "entry_out_of_arena",
        }
    }
}

const PT_LOAD: u32 = 1;

// All offset arithmetic below is checked: the header fields are treated as
// adversarial (that is the design intent — M9 feeds this loader hostile
// images), so a malformed image must become a structured Err, never a panic,
// in every build profile (debug overflow-checks included).
fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        b.get(off..off.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(off..off.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        b.get(off..off.checked_add(8)?)?.try_into().ok()?,
    ))
}

fn arena_offset(vaddr: u64) -> Option<usize> {
    let base = hal::ARENA_BASE as u64;
    if vaddr < base {
        return None;
    }
    usize::try_from(vaddr - base).ok()
}

/// Load `image` into the arena and return its entry point (a virtual/physical
/// address inside the arena; identity-mapped pre-M5). Idempotent per call —
/// the caller owns the arena for exactly one payload at a time.
pub fn load(image: &[u8]) -> Result<usize, ElfError> {
    // ELF identification.
    if image.len() < 64 {
        return Err(ElfError::TooShort);
    }
    if image.get(0..4) != Some(&[0x7f, b'E', b'L', b'F']) {
        return Err(ElfError::BadMagic);
    }
    // EI_CLASS=2 (64-bit), EI_DATA=1 (little-endian), e_machine=243 (RISC-V).
    if image[4] != 2 || image[5] != 1 {
        return Err(ElfError::NotElf64Rv);
    }
    if u16_at(image, 18) != Some(243) {
        return Err(ElfError::NotElf64Rv);
    }

    let entry = u64_at(image, 24).ok_or(ElfError::TooShort)?;
    let phoff = u64_at(image, 32).ok_or(ElfError::TooShort)?;
    let phentsize = u16_at(image, 54).ok_or(ElfError::TooShort)? as usize;
    let phnum = u16_at(image, 56).ok_or(ElfError::TooShort)? as usize;

    if phentsize < 56 {
        return Err(ElfError::BadProgramHeaders);
    }

    let phoff = usize::try_from(phoff).map_err(|_| ElfError::BadProgramHeaders)?;
    // The whole program-header table must fit in the image (also bounds every
    // `ph + delta` field read below, and rejects an overflowing table span).
    let table_span = phnum
        .checked_mul(phentsize)
        .and_then(|s| phoff.checked_add(s))
        .ok_or(ElfError::BadProgramHeaders)?;
    if table_span > image.len() {
        return Err(ElfError::BadProgramHeaders);
    }

    for i in 0..phnum {
        let ph = phoff + i * phentsize; // < table_span ≤ image.len(); no overflow
        let p_type = u32_at(image, ph).ok_or(ElfError::BadProgramHeaders)?;
        if p_type != PT_LOAD {
            continue;
        }
        let p_offset = u64_at(image, ph + 8).ok_or(ElfError::BadProgramHeaders)?;
        let p_vaddr = u64_at(image, ph + 16).ok_or(ElfError::BadProgramHeaders)?;
        let p_filesz = u64_at(image, ph + 32).ok_or(ElfError::BadProgramHeaders)?;
        let p_memsz = u64_at(image, ph + 40).ok_or(ElfError::BadProgramHeaders)?;

        let dst_off = arena_offset(p_vaddr).ok_or(ElfError::SegmentOutOfArena)?;
        let filesz = usize::try_from(p_filesz).map_err(|_| ElfError::BadProgramHeaders)?;
        let memsz = usize::try_from(p_memsz).map_err(|_| ElfError::BadProgramHeaders)?;
        if memsz < filesz {
            return Err(ElfError::BadProgramHeaders);
        }

        let src_start = usize::try_from(p_offset).map_err(|_| ElfError::BadProgramHeaders)?;
        let src_end = src_start
            .checked_add(filesz)
            .ok_or(ElfError::BadProgramHeaders)?;
        let src = image
            .get(src_start..src_end)
            .ok_or(ElfError::BadProgramHeaders)?;

        // Copy file bytes, then zero the tail (.bss lives in memsz > filesz).
        // arena_write/arena_zero bounds-check dst internally (checked_add).
        if !hal::arena_write(dst_off, src) {
            return Err(ElfError::SegmentOutOfArena);
        }
        let zero_off = dst_off
            .checked_add(filesz)
            .ok_or(ElfError::SegmentOutOfArena)?;
        if !hal::arena_zero(zero_off, memsz - filesz) {
            return Err(ElfError::SegmentOutOfArena);
        }
    }

    let entry_usize = usize::try_from(entry).map_err(|_| ElfError::EntryOutOfArena)?;
    if !(hal::ARENA_BASE..hal::ARENA_BASE + hal::ARENA_SIZE).contains(&entry_usize) {
        return Err(ElfError::EntryOutOfArena);
    }
    Ok(entry_usize)
}
