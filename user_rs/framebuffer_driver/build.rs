// Same reasoning as user_rs/serial_driver/build.rs: cargo has no
// dependency edge on linker.ld by default, so editing it alone would be
// silently ignored by an incremental build without this.
fn main() {
    println!("cargo:rerun-if-changed=linker.ld");
}
