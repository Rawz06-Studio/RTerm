//! The version RTerm displays comes from the `RTERM_VERSION` variable, which CI
//! sets from the release tag. This script only asks cargo to rebuild when it
//! changes.

fn main() {
    println!("cargo:rerun-if-env-changed=RTERM_VERSION");
    println!("cargo:rerun-if-changed=build.rs");
}
