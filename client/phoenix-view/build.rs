//! The UI thread renders the guest's tree recursively, and an unoptimized
//! build keeps every GPUI builder value of a node's frame on the stack. A
//! 1 MB main thread overflows on the login screen, so it gets 32 MB.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:33554432");
    }
}
