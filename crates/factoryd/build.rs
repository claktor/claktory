//! Builds factoryd's one piece of C: the lid-closed helper's XPC client
//! (`native/lid_xpc.c`, #371), on macOS only. XPC's handlers are blocks,
//! which C has and Rust doesn't.

fn main() {
    println!("cargo:rerun-if-changed=native/lid_xpc.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new().file("native/lid_xpc.c").flag("-fblocks").flag("-Wall").flag("-Wextra").compile("claktory_lid_xpc");
    println!("cargo:rustc-link-lib=framework=Security");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
}
