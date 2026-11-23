include!("../../scripts/build_info.rs");

fn main() {
    build_info::emit();
    // Host cfg in build.rs is the builder, not the target. Use CARGO_CFG_*.
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows_msvc {
        static_vcruntime::metabuild();
    }
}
