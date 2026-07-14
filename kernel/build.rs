// Host-side build script: hand the linker script to rust-lld with an
// absolute path, and locate the payload ELFs that get embedded via
// include_bytes! (payloads/ must be built first — `make build` does).
use std::path::Path;

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg=-T{dir}/link.ld");
    println!("cargo:rerun-if-changed=link.ld");

    let payload_dir = format!("{dir}/../payloads/target/riscv64gc-unknown-none-elf/release");
    for name in [
        "hello",
        "crasher",
        "muzzled",
        "spawner",
        "child",
        "runaway",
        "wild",
        "wxviol",
        "forker",
        "leaker",
        "delegator",
        "redelegator",
        "worker",
        "badjump",
    ] {
        let path = format!("{payload_dir}/{name}");
        if !Path::new(&path).exists() {
            panic!(
                "payload ELF missing: {path}\n\
                 payloads build before the kernel — run `make build` \
                 (or `cd payloads && cargo build --release`)"
            );
        }
        println!("cargo:rustc-env=PAYLOAD_{}={path}", name.to_uppercase());
        println!("cargo:rerun-if-changed={path}");
    }
}
