include!("../../../scripts/build_info.rs");

fn main() {
    build_info::emit();
    tauri_build::build()
}
