//! Real bug found and fixed hardening Phase 4: a large `[0u8; N]`
//! zero-init (`kernel_common::ext2`'s block buffers) is big enough for
//! LLVM to lower into a call to `memset` rather than inline stores.
//! `compiler_builtins` provides a real (weak) `memset` symbol once its
//! `mem` feature is enabled (see the workspace's various
//! `.cargo/config.toml` `build-std-features` settings) -- but on THIS
//! project's nightly-x86_64-pc-windows-gnu-HOSTED toolchain, even a
//! fully static, freestanding ELF target's calls to that symbol are
//! still emitted as an INDIRECT call through what looks like a
//! Windows-PE-style import table slot, not a direct PC-relative call —
//! a vestige of the host toolchain's own calling convention for
//! "external" functions. With `--no-dynamic-linker` (every crate in
//! this workspace passes it — there is no dynamic linker to populate
//! that slot), the indirect call target stays zero forever: a genuine
//! call-through-a-null-pointer page fault the FIRST time any code path
//! in this whole project used a buffer large enough to trigger
//! memset-lowering (`user_rs/virtio_blk_driver`'s real ext2 formatting
//! logic, this session).
//!
//! Real fix: define these as REAL, STRONG, `#[no_mangle]` symbols
//! ourselves. A symbol the compiler resolves to code defined IN the
//! same crate/binary gets a direct call, not an indirect one through an
//! unpopulated import slot — and `compiler_builtins`' own versions are
//! deliberately WEAK (exactly so a real implementation like this one
//! can override them). Every freestanding crate in this workspace
//! (`kernel_rs` and all four `user_rs/*_driver` crates) depends on
//! `kernel_common` already or now does, so this one definition covers
//! all of them; each produces its own separate final binary, so there's
//! no cross-binary symbol collision risk.
//!
//! Plain byte-at-a-time loops, deliberately not vectorized/optimized —
//! correctness over speed for a function that exists specifically to
//! avoid a linker/codegen quirk, not to be a fast general-purpose libc.

use core::ffi::c_void;

// Real libc signatures (c_void, not u8) -- rustc's own "suspicious
// runtime symbol" lint checks these against the standard library's
// expectations for these specific well-known names; matching them
// exactly (not just something ABI-compatible) silences it for real
// instead of papering over a genuine mismatch.
#[no_mangle]
pub unsafe extern "C" fn memset(dest: *mut c_void, value: i32, count: usize) -> *mut c_void {
    let d = dest as *mut u8;
    let v = value as u8;
    let mut i = 0;
    while i < count {
        *d.add(i) = v;
        i += 1;
    }
    dest
}

#[no_mangle]
pub unsafe extern "C" fn memcpy(dest: *mut c_void, src: *const c_void, count: usize) -> *mut c_void {
    let (d, s) = (dest as *mut u8, src as *const u8);
    let mut i = 0;
    while i < count {
        *d.add(i) = *s.add(i);
        i += 1;
    }
    dest
}

#[no_mangle]
pub unsafe extern "C" fn memmove(dest: *mut c_void, src: *const c_void, count: usize) -> *mut c_void {
    let (d, s) = (dest as *mut u8, src as *const u8);
    if (d as usize) < (s as usize) {
        let mut i = 0;
        while i < count {
            *d.add(i) = *s.add(i);
            i += 1;
        }
    } else {
        let mut i = count;
        while i > 0 {
            i -= 1;
            *d.add(i) = *s.add(i);
        }
    }
    dest
}

#[no_mangle]
pub unsafe extern "C" fn memcmp(a: *const c_void, b: *const c_void, count: usize) -> i32 {
    let (a, b) = (a as *const u8, b as *const u8);
    let mut i = 0;
    while i < count {
        let (av, bv) = (*a.add(i), *b.add(i));
        if av != bv {
            return av as i32 - bv as i32;
        }
        i += 1;
    }
    0
}
