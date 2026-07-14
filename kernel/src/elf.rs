#![forbid(unsafe_code)]
//! Minimal ELF64 loader for rv64 payloads. Parses a byte slice and maps each
//! PT_LOAD segment into a fresh per-payload address space (M5): a frame per
//! page, file bytes copied in, the rest left zero (`.bss`), mapped at the
//! segment's virtual address with permissions taken from the program header —
//! so a writable segment is never executable (W^X). Untrusted image bytes are
//! handled entirely in safe code (checked arithmetic, `hal::phys_*`).
//!
//! No dynamic linking, no relocations (payloads are statically linked at a
//! fixed low VA); those are hard non-goals.

use crate::mm::{self, AddressSpace};

/// Why an image could not be loaded. These become fields in the load-fault
/// diagnostic frame (P6) rather than panics.
#[derive(Clone, Copy)]
pub enum ElfError {
    TooShort,
    BadMagic,
    NotElf64Rv,
    BadProgramHeaders,
    WxViolation,
    OutOfMemory,
    MapFailed,
    EntryUnmapped,
}

impl ElfError {
    pub fn as_str(self) -> &'static str {
        match self {
            ElfError::TooShort => "too_short",
            ElfError::BadMagic => "bad_magic",
            ElfError::NotElf64Rv => "not_elf64_riscv",
            ElfError::BadProgramHeaders => "bad_program_headers",
            ElfError::WxViolation => "wx_violation",
            ElfError::OutOfMemory => "out_of_memory",
            ElfError::MapFailed => "map_failed",
            ElfError::EntryUnmapped => "entry_unmapped",
        }
    }
}

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

// Checked reads: header fields are treated as adversarial (M9 feeds this
// loader hostile images), so a malformed image is a structured Err, never a
// panic, in every build profile.
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

/// Load `image` into `space` and return its entry virtual address. On error
/// the caller destroys `space` (freeing any frames already mapped).
pub fn load(image: &[u8], space: &AddressSpace) -> Result<usize, ElfError> {
    if image.len() < 64 {
        return Err(ElfError::TooShort);
    }
    if image.get(0..4) != Some(&[0x7f, b'E', b'L', b'F']) {
        return Err(ElfError::BadMagic);
    }
    // EI_CLASS=2 (64-bit), EI_DATA=1 (little-endian), e_machine=243 (RISC-V).
    if image[4] != 2 || image[5] != 1 || u16_at(image, 18) != Some(243) {
        return Err(ElfError::NotElf64Rv);
    }

    let entry = u64_at(image, 24).ok_or(ElfError::TooShort)? as usize;
    let phoff = usize::try_from(u64_at(image, 32).ok_or(ElfError::TooShort)?)
        .map_err(|_| ElfError::BadProgramHeaders)?;
    let phentsize = u16_at(image, 54).ok_or(ElfError::TooShort)? as usize;
    let phnum = u16_at(image, 56).ok_or(ElfError::TooShort)? as usize;
    if phentsize < 56 {
        return Err(ElfError::BadProgramHeaders);
    }
    let table_span = phnum
        .checked_mul(phentsize)
        .and_then(|s| phoff.checked_add(s))
        .ok_or(ElfError::BadProgramHeaders)?;
    if table_span > image.len() {
        return Err(ElfError::BadProgramHeaders);
    }

    for i in 0..phnum {
        let ph = phoff + i * phentsize; // < table_span ≤ len
        if u32_at(image, ph).ok_or(ElfError::BadProgramHeaders)? != PT_LOAD {
            continue;
        }
        let p_flags = u32_at(image, ph + 4).ok_or(ElfError::BadProgramHeaders)?;
        let p_offset = usize::try_from(u64_at(image, ph + 8).ok_or(ElfError::BadProgramHeaders)?)
            .map_err(|_| ElfError::BadProgramHeaders)?;
        let p_vaddr = usize::try_from(u64_at(image, ph + 16).ok_or(ElfError::BadProgramHeaders)?)
            .map_err(|_| ElfError::BadProgramHeaders)?;
        let p_filesz = usize::try_from(u64_at(image, ph + 32).ok_or(ElfError::BadProgramHeaders)?)
            .map_err(|_| ElfError::BadProgramHeaders)?;
        let p_memsz = usize::try_from(u64_at(image, ph + 40).ok_or(ElfError::BadProgramHeaders)?)
            .map_err(|_| ElfError::BadProgramHeaders)?;
        if p_memsz < p_filesz {
            return Err(ElfError::BadProgramHeaders);
        }

        let mut perms = mm::U;
        if p_flags & PF_R != 0 {
            perms |= mm::R;
        }
        if p_flags & PF_W != 0 {
            perms |= mm::W;
        }
        if p_flags & PF_X != 0 {
            perms |= mm::X;
        }
        // W^X: a segment must never be both writable and executable.
        if perms & mm::W != 0 && perms & mm::X != 0 {
            return Err(ElfError::WxViolation);
        }

        map_segment(image, space, p_vaddr, p_offset, p_filesz, p_memsz, perms)?;
    }

    if space.translate(entry).is_none() {
        return Err(ElfError::EntryUnmapped);
    }
    Ok(entry)
}

#[allow(clippy::too_many_arguments)]
fn map_segment(
    image: &[u8],
    space: &AddressSpace,
    vaddr: usize,
    offset: usize,
    filesz: usize,
    memsz: usize,
    perms: u64,
) -> Result<(), ElfError> {
    let seg_end = vaddr
        .checked_add(memsz)
        .ok_or(ElfError::BadProgramHeaders)?;
    let file_end = vaddr
        .checked_add(filesz)
        .ok_or(ElfError::BadProgramHeaders)?;
    let mut page = vaddr - (vaddr % mm::PAGE_SIZE);
    while page < seg_end {
        let frame = crate::frames::alloc().ok_or(ElfError::OutOfMemory)?;
        // Copy the file bytes that fall within this page (the rest of the
        // frame is already zero — fresh frames are zeroed — giving free .bss).
        let copy_start = page.max(vaddr);
        let copy_end = (page + mm::PAGE_SIZE).min(file_end);
        if copy_start < copy_end {
            let src_off = offset + (copy_start - vaddr);
            let src = image
                .get(src_off..src_off + (copy_end - copy_start))
                .ok_or(ElfError::BadProgramHeaders)?;
            if !crate::hal::phys_write(frame + (copy_start - page), src) {
                crate::frames::free(frame);
                return Err(ElfError::MapFailed);
            }
        }
        space.map_page(page, frame, perms).map_err(|_| {
            crate::frames::free(frame);
            ElfError::MapFailed
        })?;
        page += mm::PAGE_SIZE;
    }
    Ok(())
}
