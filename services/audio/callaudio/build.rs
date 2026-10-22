//! Compiles the narrow PipeWire and alsa-lib adapter; the policy stays in Rust.
use std::process::Command;

fn main() {
    let flags = Command::new("pkg-config")
        .args(["--cflags", "libpipewire-0.3", "alsa"])
        .output()
        .expect("pkg-config is required for the PipeWire and ALSA headers");
    assert!(
        flags.status.success(),
        "libpipewire-0.3 and alsa development files are required"
    );
    let mut build = cc::Build::new();
    build
        .file("src/native.c")
        .flag("-std=c11")
        .flag("-D_GNU_SOURCE")
        // PipeWire callbacks have fixed signatures.
        .flag("-Wno-unused-parameter");
    for flag in String::from_utf8(flags.stdout).unwrap().split_whitespace() {
        build.flag(flag);
    }
    build.warnings(true).compile("callaudio_native");
    println!("cargo:rustc-link-lib=pipewire-0.3");
    println!("cargo:rustc-link-lib=asound");
    println!("cargo:rerun-if-changed=src/native.c");
}
