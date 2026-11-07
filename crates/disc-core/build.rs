fn main() {
    println!("cargo:rerun-if-changed=native/macos_scsi.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new().file("native/macos_scsi.c").flag("-std=c11").warnings(true).compile("macos_scsi");
        println!("cargo:rustc-link-lib=framework=IOKit");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
    }
}
