#![forbid(unsafe_code)]
//! Sv39 page tables (three levels, 4 KiB pages, 2 MiB/1 GiB superpages).
//!
//! Every payload gets its own address space: its ELF segments and stack live
//! at low virtual addresses (user-accessible, W^X per segment), and the
//! kernel's RAM is a single supervisor gigapage (U=0) so the trap handler —
//! which runs with the payload's `satp` active — can execute, while a U-mode
//! payload that touches it faults. That is the M5 isolation guarantee.
//!
//! All page-table memory lives in the frame pool and is read/written only
//! through the bounds-checked `hal::phys_*` accessors, so this stays safe
//! code (no raw pointers, no `unsafe`).

use crate::{frames, hal};

pub const PAGE_SIZE: usize = 4096;

// PTE flag bits.
pub const V: u64 = 1 << 0; // valid
pub const R: u64 = 1 << 1; // readable
pub const W: u64 = 1 << 2; // writable
pub const X: u64 = 1 << 3; // executable
pub const U: u64 = 1 << 4; // user-accessible
pub const G: u64 = 1 << 5; // global
const A: u64 = 1 << 6; // accessed
const D: u64 = 1 << 7; // dirty
const PERM_MASK: u64 = R | W | X;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmError {
    NoFrame,   // pool exhausted
    Overlap,   // VA already mapped
    Unaligned, // superpage PA/VA not aligned
}

/// A completed address space: the root table PPN, ready to load into satp.
#[derive(Clone, Copy)]
pub struct AddressSpace {
    root: usize, // physical address of the root table frame
}

/// Virtual (== physical) address of the kernel identity gigapage. All of
/// qemu-virt's DRAM above the firmware falls in this 1 GiB window.
pub const KERNEL_GIGAPAGE_VA: usize = 0x8000_0000;

/// Add the kernel identity map (a supervisor gigapage) to `space`. Every
/// payload table carries it so the trap handler — which runs with the
/// payload's satp active — can execute; U=0 means a U-mode payload that
/// touches kernel RAM faults (the M5 isolation guarantee).
pub fn map_kernel(space: &AddressSpace) -> Result<(), MmError> {
    space.map_gigapage(KERNEL_GIGAPAGE_VA, KERNEL_GIGAPAGE_VA, R | W | X | G)
}

impl AddressSpace {
    /// Allocate an empty address space (a zeroed root table).
    pub fn new() -> Option<Self> {
        Some(AddressSpace {
            root: frames::alloc()?,
        })
    }

    /// Reconstruct a handle to an existing address space from its root frame.
    pub fn from_root(root: usize) -> Self {
        AddressSpace { root }
    }

    pub fn root(&self) -> usize {
        self.root
    }

    /// satp value that activates this address space (MODE=8 Sv39, ASID=0).
    pub fn satp(&self) -> u64 {
        (8u64 << 60) | ((self.root as u64) >> 12)
    }

    /// Map one 4 KiB page `va -> pa` with `perms` (R/W/X/U bits; V/A/D added).
    pub fn map_page(&self, va: usize, pa: usize, perms: u64) -> Result<(), MmError> {
        let mut table = self.root;
        for level in [2usize, 1usize] {
            let slot = table + vpn(va, level) * 8;
            let pte = read_pte(slot);
            if pte & V == 0 {
                let next = frames::alloc().ok_or(MmError::NoFrame)?;
                write_pte(slot, nonleaf(next));
                table = next;
            } else if pte & PERM_MASK != 0 {
                return Err(MmError::Overlap); // a superpage already covers this VA
            } else {
                table = pte_pa(pte);
            }
        }
        let slot = table + vpn(va, 0) * 8;
        if read_pte(slot) & V != 0 {
            return Err(MmError::Overlap);
        }
        write_pte(slot, leaf(pa, perms));
        Ok(())
    }

    /// Map a 1 GiB gigapage (level-2 leaf). Used for the kernel identity map.
    pub fn map_gigapage(&self, va: usize, pa: usize, perms: u64) -> Result<(), MmError> {
        const GIB: usize = 1 << 30;
        if !va.is_multiple_of(GIB) || !pa.is_multiple_of(GIB) {
            return Err(MmError::Unaligned);
        }
        let slot = self.root + vpn(va, 2) * 8;
        if read_pte(slot) & V != 0 {
            return Err(MmError::Overlap);
        }
        write_pte(slot, leaf(pa, perms));
        Ok(())
    }

    /// Translate a user VA to a physical address, if mapped.
    pub fn translate(&self, va: usize) -> Option<usize> {
        let mut table = self.root;
        for level in [2usize, 1usize, 0usize] {
            let pte = read_pte(table + vpn(va, level) * 8);
            if pte & V == 0 {
                return None;
            }
            if pte & PERM_MASK != 0 {
                // leaf at this level: combine with the in-page offset
                let page = 1usize << (12 + 9 * level);
                return Some(pte_pa(pte) + (va & (page - 1)));
            }
            table = pte_pa(pte);
        }
        None
    }

    /// Copy `len` bytes from user VA `va` into `buf` (for the write syscall).
    /// Walks page by page; false if any page is unmapped.
    pub fn copy_from_user(&self, va: usize, buf: &mut [u8]) -> bool {
        let mut done = 0;
        while done < buf.len() {
            let cur = va + done;
            let pa = match self.translate(cur) {
                Some(pa) => pa,
                None => return false,
            };
            let page_left = PAGE_SIZE - (cur % PAGE_SIZE);
            let n = page_left.min(buf.len() - done);
            if !hal::phys_read(pa, &mut buf[done..done + n]) {
                return false;
            }
            done += n;
        }
        true
    }

    /// Walk the table for `va`, returning each level's PTE until the leaf or
    /// the first invalid entry — the seed of the P6 page-table-walk frame.
    /// Returns (level, pte) pairs, outermost first.
    pub fn walk(&self, va: usize) -> WalkChain {
        let mut chain = WalkChain::default();
        let mut table = self.root;
        for level in [2usize, 1usize, 0usize] {
            let pte = read_pte(table + vpn(va, level) * 8);
            chain.push(level as u8, pte);
            if pte & V == 0 || pte & PERM_MASK != 0 {
                break; // invalid, or a leaf: walk stops
            }
            table = pte_pa(pte);
        }
        chain
    }

    /// Free every user frame and intermediate table this space owns, then the
    /// root. Kernel gigapages (supervisor leaves at the root) are left alone —
    /// their target is shared kernel RAM, not owned by this payload.
    pub fn destroy(self) {
        for i in 0..512 {
            let slot = self.root + i * 8;
            let pte = read_pte(slot);
            if pte & V == 0 || pte & PERM_MASK != 0 {
                continue; // empty, or a kernel gigapage — don't recurse/free
            }
            free_subtree(pte_pa(pte), 1);
        }
        frames::free(self.root);
    }
}

/// Up to three (level, pte) pairs from a page-table walk.
#[derive(Default, Clone, Copy)]
pub struct WalkChain {
    levels: [u8; 3],
    ptes: [u64; 3],
    len: usize,
}

impl WalkChain {
    fn push(&mut self, level: u8, pte: u64) {
        if self.len < 3 {
            self.levels[self.len] = level;
            self.ptes[self.len] = pte;
            self.len += 1;
        }
    }
    pub fn entries(&self) -> impl Iterator<Item = (u8, u64)> + '_ {
        (0..self.len).map(move |i| (self.levels[i], self.ptes[i]))
    }
}

fn free_subtree(table: usize, level: usize) {
    for i in 0..512 {
        let pte = read_pte(table + i * 8);
        if pte & V == 0 {
            continue;
        }
        if pte & PERM_MASK != 0 {
            // leaf: free the user data frame (only user frames reach here —
            // this subtree is entirely payload-owned).
            frames::free(pte_pa(pte));
        } else if level > 0 {
            free_subtree(pte_pa(pte), level - 1);
        }
    }
    frames::free(table);
}

// ---- PTE helpers --------------------------------------------------------

fn vpn(va: usize, level: usize) -> usize {
    (va >> (12 + 9 * level)) & 0x1ff
}
fn read_pte(pa: usize) -> u64 {
    hal::phys_read_u64(pa).unwrap_or(0)
}
fn write_pte(pa: usize, val: u64) {
    hal::phys_write_u64(pa, val);
}
fn pte_pa(pte: u64) -> usize {
    (((pte >> 10) & 0xfff_ffff_ffff) << 12) as usize
}
fn nonleaf(pa: usize) -> u64 {
    ((pa as u64) >> 12) << 10 | V
}
fn leaf(pa: usize, perms: u64) -> u64 {
    ((pa as u64) >> 12) << 10 | perms | V | A | D
}
