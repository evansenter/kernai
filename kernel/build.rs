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
        "breaker",
        "misalign",
        "nullread",
        "execdata",
        "stackover",
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

    // The C payloads (cpayloads feature) are built by payloads/build_c.sh into
    // the same dir; only require them when the feature asked for them.
    if std::env::var("CARGO_FEATURE_CPAYLOADS").is_ok() {
        let path = format!("{payload_dir}/craycast");
        if !Path::new(&path).exists() {
            panic!(
                "C payload ELF missing: {path}\n\
                 the cpayloads feature needs the C payloads built first — \
                 run `make raycast` (or payloads/build_c.sh)"
            );
        }
        println!("cargo:rustc-env=PAYLOAD_CRAYCAST={path}");
        println!("cargo:rerun-if-changed={path}");
    }

    // The DOOM payload (doom feature), built by payloads/build_doom.sh into the
    // same dir; only required when the feature asks for it.
    if std::env::var("CARGO_FEATURE_DOOM").is_ok() {
        let path = format!("{payload_dir}/doom");
        if !Path::new(&path).exists() {
            panic!(
                "DOOM payload ELF missing: {path}\n\
                 the doom feature needs the DOOM payload built first — \
                 run `make doom` (or payloads/build_doom.sh)"
            );
        }
        println!("cargo:rustc-env=PAYLOAD_DOOM={path}");
        println!("cargo:rerun-if-changed={path}");
    }
}
