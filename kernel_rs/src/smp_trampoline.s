# Real AP bring-up trampoline (docs/ROADMAP.md Phase 9, deliverable 1).
# Assembled into kernel_rs's own binary via `global_asm!(include_str!(...))`
# in smp.rs, then copied BYTE-FOR-BYTE by Rust to a fixed physical page
# (TRAMPOLINE_PHYS, 0x8000 -- see smp.rs) BEFORE any INIT-SIPI-SIPI is
# ever sent. Real x86 CPU bring-up mechanics, not a simplification: a
# just-SIPI'd AP starts executing real, honest-to-hardware 16-bit real
# mode code at CS:IP = (SIPI vector << 8):0x0000 -- there is no way
# around that, no matter what mode the BSP itself is already running
# in. This walks it real-mode -> 32-bit protected mode -> 64-bit long
# mode, landing in this kernel's OWN real page tables and a real Rust
# function (`ap_entry`, smp.rs), never a simulated/faked transition.
#
# Every absolute address below is a compile-time constant computed as
# TRAMPOLINE_BASE + (label - ap_trampoline_start) -- deliberately NOT
# relying on this .s file's own link-time address (which is wherever
# the normal higher-half kernel linker puts it, irrelevant here) and
# NOT needing any runtime relocation/patching either, because Rust
# guarantees this blob is always copied to physical TRAMPOLINE_BASE
# before it ever executes. Three small DATA fields (entry point,
# target PML4, stack top) are left as fixed offsets near the end of
# the same page for smp.rs to fill in, per-AP, before each SIPI.

.set TRAMPOLINE_BASE, 0x8000
.set DATA_ENTRY_OFF,  0xFE0   # u64: real higher-half ap_entry() address
.set DATA_PML4_OFF,   0xFE8   # u64: kernel_pml4_phys (low 32 bits used)
.set DATA_STACK_OFF,  0xFF0   # u64: this AP's own real64-bit RSP (direct-map vaddr)

.section .text
.global ap_trampoline_start
.global ap_trampoline_end

.code16
ap_trampoline_start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax

    # LLVM's integrated assembler refuses a `sym - sym` expression
    # written directly inside a `[...]` memory operand or after a far
    # jump's `seg:` -- precomputed `.set` constants below (GDT32PTR_ADDR
    # / PM32_TARGET / LM64_TARGET) work around that; the resulting
    # VALUES are identical, this is purely an assembler-syntax
    # accommodation, not a design change.
    lgdt [GDT32PTR_ADDR]

    mov eax, cr0
    or eax, 1                      # CR0.PE
    mov cr0, eax

    # Far jump into 32-bit protected mode -- flushes the prefetch queue
    # and loads CS with the flat code32 descriptor below (selector 0x08).
    # Hand-encoded raw opcode (0xEA = JMP ptr16:16, 16-bit default
    # operand size here in .code16 context) rather than a `jmp`/`ljmp`
    # mnemonic -- LLVM's integrated assembler (used by global_asm!)
    # rejects every symbolic mnemonic form of a far jump this file
    # tried; the raw bytes are unambiguous and dialect-independent.
    # PM32_TARGET fits in 16 bits (TRAMPOLINE_BASE=0x8000 plus a small
    # offset), so the 16-bit-offset encoding is exact, not truncated.
    .byte 0xEA
    .word PM32_TARGET
    .word 0x08

.code32
pm32_entry:
    mov ax, 0x10                   # flat data32 selector
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    # A tiny, temporary stack -- only a handful of instructions run
    # before this AP is on its own REAL stack (DATA_STACK_OFF, set up
    # below) in 64-bit mode; this one only needs to survive that gap.
    mov esp, TRAMPOLINE_BASE + 0xF00

    # CR4.PAE -- required before EFER.LME can take effect.
    mov eax, cr4
    or eax, (1 << 5)
    mov cr4, eax

    # CR3 = this kernel's REAL page tables (the SAME ones the BSP
    # already uses -- see smp.rs's own doc for why the low identity
    # mapping this depends on lives in that SAME shared PML4, not a
    # separate/temporary one). Low 32 bits only: a real, stated
    # assumption -- this kernel's PML4 has always been allocated well
    # within 32 bits of physical address on every target exercised so
    # far (QEMU, a few hundred MB of RAM), not silently relied on
    # without saying so.
    mov eax, [TRAMPOLINE_BASE + DATA_PML4_OFF]
    mov cr3, eax

    # EFER.LME AND EFER.NXE. EFER is per-core state (NOT shared with the
    # BSP) -- this kernel's own page tables (vmm.rs) set the NX bit
    # (bit 63) on essentially every data mapping, including the direct-
    # map window this AP needs immediately. Setting LME alone and
    # jumping into those same page tables with THIS core's own NXE
    # still off is a real, found-by-testing bug: bit 63 becomes a
    # RESERVED bit (not a valid NX bit) to a core with NXE=0, and the
    # very first access to an NX-tagged page raises a reserved-bit #PF
    # (error code 0x0008) -- confirmed via QEMU's own `-d int` trace on
    # the first real run of this trampoline, not guessed: a genuine
    # page fault at a kernel .data address, immediately cascading to a
    # double fault (no per-AP IDT exists yet either) and then a triple
    # fault. NXE (bit 11) alongside LME (bit 8) fixes it.
    mov ecx, 0xC0000080
    rdmsr
    or eax, (1 << 8) | (1 << 11)
    wrmsr

    # CR0.PG -- the instant this executes, the CPU starts translating
    # EVERY subsequent fetch through the page tables just loaded above.
    # The very next instruction is still physically at
    # TRAMPOLINE_BASE+something -- it stays fetchable ONLY because
    # smp.rs identity-mapped this exact physical page into that same
    # PML4 before ever sending SIPI. Without that mapping this is a
    # real, immediate triple fault, not a hang -- see smp.rs's doc.
    mov eax, cr0
    or eax, (1 << 31)
    mov cr0, eax

    # Far jump to the code64 descriptor (selector 0x18) -- this is what
    # actually switches CPU submode from IA-32e compatibility to real
    # 64-bit long mode (loading a code descriptor with the L-bit set is
    # what does it, not CR0.PG alone). Raw-encoded (0xEA, same reasoning
    # as the jump above); 32-bit-offset form since we're in .code32
    # context here, still exact since LM64_TARGET fits in 16 bits.
    .byte 0xEA
    .long LM64_TARGET
    .word 0x18

.code64
lm64_entry:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    # This AP's real, dedicated 64-bit stack (a direct-map-window
    # virtual address smp.rs computed and wrote into this exact
    # physical offset before SIPI -- reachable now because the kernel's
    # real PML4, already loaded, has always mapped its own direct-map
    # window over all reported physical RAM since vmm::init()).
    mov rsp, [TRAMPOLINE_BASE + DATA_STACK_OFF]

    # Jump into the real, higher-half Rust entry point. Never returns.
    mov rax, [TRAMPOLINE_BASE + DATA_ENTRY_OFF]
    jmp rax

# A minimal, throwaway 32/64-bit GDT -- used ONLY to make this specific
# real-mode -> protected-mode -> long-mode transition; `ap_entry` (Rust)
# reloads a REAL GDT (the kernel's own, shared, read-only from here) as
# one of its first acts, once it's safely running in higher-half kernel
# code -- see smp.rs's doc for why this trampoline never touches that
# shared GDT/TSS itself.
.align 16
gdt32_start:
    .quad 0                                    # null
    .word 0xFFFF, 0x0000
    .byte 0x00, 0x9A, 0xCF, 0x00                # 0x08: code32, base=0 limit=4G, 32-bit
    .word 0xFFFF, 0x0000
    .byte 0x00, 0x92, 0xCF, 0x00                # 0x10: data32, base=0 limit=4G
    .word 0xFFFF, 0x0000
    .byte 0x00, 0x9A, 0xAF, 0x00                # 0x18: code64 (L-bit set, D=0)
gdt32_end:

.align 4
gdt32_ptr:
    .word gdt32_end - gdt32_start - 1
    .long GDT32_TABLE_ADDR

ap_trampoline_end:

# Precomputed absolute addresses -- see the note above `lgdt` for why
# these are `.set` constants rather than inline expressions. Defined
# after the labels they reference (GAS/LLVM resolve same-file symbol
# arithmetic regardless of definition order); each is just
# TRAMPOLINE_BASE + (label's byte offset from ap_trampoline_start), a
# real compile-time constant -- nothing here is a runtime relocation.
.set GDT32PTR_ADDR,  TRAMPOLINE_BASE + (gdt32_ptr   - ap_trampoline_start)
.set GDT32_TABLE_ADDR, TRAMPOLINE_BASE + (gdt32_start - ap_trampoline_start)
.set PM32_TARGET,    TRAMPOLINE_BASE + (pm32_entry  - ap_trampoline_start)
.set LM64_TARGET,    TRAMPOLINE_BASE + (lm64_entry  - ap_trampoline_start)
