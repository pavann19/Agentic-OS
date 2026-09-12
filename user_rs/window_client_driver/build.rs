// Same reasoning as the other user_rs/*/build.rs files: cargo has no
// dependency edge on linker.ld by default.
fn main() {
    println!("cargo:rerun-if-changed=linker.ld");
}
