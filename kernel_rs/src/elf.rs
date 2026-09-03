//! ELF64 loader for user-space programs. Phase 3's "first user-space
//! drivers" item needs this specifically: every ring-3 process before
//! this one (`init.rs`, the Phase 1/2 `demo_ring3` proof) has been a
//! hand-built machine-code byte array copied directly into a mapped
//! page — that doesn't scale to porting a real driver written as normal
//! Rust code. This is a real ELF64 parser/loader (not a stub): validates
//! the header, walks PT_LOAD program headers, and maps each one into a
//! target address space with the SAME real permission discipline
//! `driver.rs::map_mmio` and every other mapper in this kernel uses
//! (`PAGE_USER` always, `PAGE_WRITABLE` iff `PF_W`, `PAGE_NO_EXECUTE`
//! unless `PF_X`).
//!
//! Field layout mirrors `boot_rs/src/elf.rs` (itself a port of
//! `boot/elf.h`) exactly — same ELF64 subset, Ehdr/Phdr, PT_LOAD only.
//! Deliberately not shared as one crate: `boot_rs` parses under UEFI
//! BootServices allocations, this parses under the kernel's own PMM/VMM;
//! keeping them textually separate avoids coupling two very differently
//! constrained callers to one abstraction neither fully needs.
//!
//! Honesty note on scope: there is no filesystem yet (Phase 4, not
//! started), so there is nowhere on "disk" for a user ELF to come from.
//! The one caller of this loader today (`init.rs`'s serial driver spawn)
//! embeds its ELF bytes at kernel build time via `include_bytes!` — a
//! real, honest interim source, not a simulated one. This loader itself
//! doesn't care where the bytes came from; swapping in a real filesystem
//! read later is a caller-side change, not a change here.

use crate::{pmm, vmm};

const EI_NIDENT: usize = 16;
const PT_LOAD: u32 = 1;
const EM_X86_64: u16 = 62;
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EV_CURRENT: u8 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Ehdr {
    e_ident: [u8; EI_NIDENT],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

#[derive(Debug)]
pub enum ElfError {
    TooSmall,
    BadMagic,
    WrongClassOrEndian,
    WrongMachineOrVersion,
    PhdrOutOfBounds,
    SegmentOutOfBounds,
}

/// Parses `bytes` as an ELF64 x86_64 static executable, maps every
/// PT_LOAD segment into `into_pml4` at its real `p_vaddr` (page-aligned
/// down, `p_memsz` page-aligned up — covering true BSS, not just the
/// file-backed portion), and returns the real entry point from the
/// header. No relocation processing (matches `boot_rs`'s loader and this
/// kernel's own `relocation-model=static` build convention — see
/// `.cargo/config.toml`'s comment on why: the loader never processes
/// relocations, so a self-relocating binary would be silently wrong).
pub unsafe fn load(into_pml4: u64, bytes: &[u8]) -> Result<u64, ElfError> {
    if bytes.len() < core::mem::size_of::<Elf64Ehdr>() {
        return Err(ElfError::TooSmall);
    }
    let ehdr = core::ptr::read_unaligned(bytes.as_ptr() as *const Elf64Ehdr);
    if &ehdr.e_ident[0..4] != b"\x7fELF" {
        return Err(ElfError::BadMagic);
    }
    if ehdr.e_ident[4] != ELFCLASS64 || ehdr.e_ident[5] != ELFDATA2LSB {
        return Err(ElfError::WrongClassOrEndian);
    }
    if ehdr.e_machine != EM_X86_64 || ehdr.e_version != EV_CURRENT as u32 {
        return Err(ElfError::WrongMachineOrVersion);
    }
    if ehdr.e_phentsize as usize != core::mem::size_of::<Elf64Phdr>() {
        return Err(ElfError::PhdrOutOfBounds);
    }

    let phoff = ehdr.e_phoff as usize;
    let phnum = ehdr.e_phnum as usize;
    let phdr_bytes_len = phnum * core::mem::size_of::<Elf64Phdr>();
    if phoff.checked_add(phdr_bytes_len).map_or(true, |end| end > bytes.len()) {
        return Err(ElfError::PhdrOutOfBounds);
    }

    for i in 0..phnum {
        let off = phoff + i * core::mem::size_of::<Elf64Phdr>();
        let ph = core::ptr::read_unaligned(bytes.as_ptr().add(off) as *const Elf64Phdr);
        if ph.p_type != PT_LOAD {
            continue;
        }
        let file_start = ph.p_offset as usize;
        let file_end = file_start
            .checked_add(ph.p_filesz as usize)
            .ok_or(ElfError::SegmentOutOfBounds)?;
        if file_end > bytes.len() || ph.p_filesz > ph.p_memsz {
            return Err(ElfError::SegmentOutOfBounds);
        }

        let vaddr_start = ph.p_vaddr & !0xFFF;
        let vaddr_end = (ph.p_vaddr + ph.p_memsz + 0xFFF) & !0xFFF;
        let pages = (vaddr_end - vaddr_start) / 4096;

        let writable = ph.p_flags & PF_W != 0;
        let executable = ph.p_flags & PF_X != 0;
        let mut flags = vmm::PAGE_USER;
        if writable {
            flags |= vmm::PAGE_WRITABLE;
        }
        if !executable {
            flags |= vmm::PAGE_NO_EXECUTE;
        }

        for p in 0..pages {
            let page_vaddr = vaddr_start + p * 4096;
            let page_phys = pmm::alloc_page();
            let page_ptr = pmm::p2v_pub(page_phys);
            core::ptr::write_bytes(page_ptr, 0, 4096);

            // Copy whatever real file bytes overlap this page -- p_vaddr
            // isn't necessarily page-aligned itself, so the FIRST page of
            // a segment may need a sub-page offset into both the file
            // buffer and the destination page.
            let page_vaddr_end = page_vaddr + 4096;
            let seg_vaddr_start = ph.p_vaddr;
            let seg_vaddr_file_end = ph.p_vaddr + ph.p_filesz;
            let copy_start = core::cmp::max(page_vaddr, seg_vaddr_start);
            let copy_end = core::cmp::min(page_vaddr_end, seg_vaddr_file_end);
            if copy_start < copy_end {
                let dst_off = (copy_start - page_vaddr) as usize;
                let src_off = file_start + (copy_start - seg_vaddr_start) as usize;
                let len = (copy_end - copy_start) as usize;
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr().add(src_off),
                    page_ptr.add(dst_off),
                    len,
                );
            }

            vmm::map_page_in(into_pml4, page_vaddr, page_phys, flags);
        }
    }

    Ok(ehdr.e_entry)
}
