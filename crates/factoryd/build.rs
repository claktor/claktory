//! Builds factoryd's one piece of C, on macOS only: the kernel's power
//! manager as far as a laptop's lid goes (`native/clamshell.c`, #371).

fn main() {
    println!("cargo:rerun-if-changed=native/clamshell.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new().file("native/clamshell.c").flag("-Wall").flag("-Wextra").compile("claktory_clamshell");
    println!("cargo:rustc-link-lib=framework=IOKit");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
}
