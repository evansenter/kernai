// Link at the kernel's payload arena (payloads/link.ld), absolute path so
// builds work from any working directory.
fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg=-T{dir}/../link.ld");
    println!("cargo:rerun-if-changed=../link.ld");
}
