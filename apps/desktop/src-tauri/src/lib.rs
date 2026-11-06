/// Desktop shell. The shop screens live in the Expo web export, not here.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running OpenBazaar desktop");
}
