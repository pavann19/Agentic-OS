//! Real cycle-accurate timestamps via `RDTSC`, for latency measurements
//! taken from ring 3. Same instruction `kernel_rs::compositor_metrics::
//! read_tsc` already uses in ring 0 for GUI frame timing -- one
//! canonical ring-3 copy here instead of each app hand-rolling its own
//! `rdtsc` inline asm.

#[inline(always)]
pub fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    ((hi as u64) << 32) | (lo as u64)
}
