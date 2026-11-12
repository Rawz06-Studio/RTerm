//! - The version RTerm displays comes from the `RTERM_VERSION` variable, which
//!   CI sets from the release tag: cargo rebuilds when it changes.
//! - On Windows, the icon and the version are embedded in RTerm.exe.

fn main() {
    println!("cargo:rerun-if-env-changed=RTERM_VERSION");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let version = std::env::var("RTERM_VERSION")
            .unwrap_or_else(|_| format!("{}-dev", std::env::var("CARGO_PKG_VERSION").unwrap_or_default()));
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/icon.ico")
            .set("FileDescription", "RTerm")
            .set("ProductName", "RTerm")
            .set("ProductVersion", &version)
            .set("LegalCopyright", "MIT License");
        resource.compile().expect("failed to embed the Windows icon");
    }
}
