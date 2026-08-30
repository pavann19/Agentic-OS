// Ensures cargo re-links when linker.ld changes -- without this, cargo has
// no dependency edge on the linker script (only on source files), so
// editing linker.ld alone (as happened fixing the page-alignment bug) is
// silently ignored by an incremental build unless something else also
// forces a relink. Same pattern kernel_rs/boot_rs would want too, but
// their linker scripts haven't needed editing since this was noticed.
fn main() {
    println!("cargo:rerun-if-changed=linker.ld");
}
