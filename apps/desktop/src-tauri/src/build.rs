//! Which build of the shell this is, set by `scripts/build_info.rs` the same
//! way as ob-node's, so the shop can tell when the bundled node is another build.

use tauri::menu::Menu;
use tauri::{AppHandle, Runtime};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = env!("OB_BUILD_COMMIT");
pub const DIRTY: bool = matches!(env!("OB_BUILD_DIRTY").as_bytes(), b"true");
pub const DATE: &str = env!("OB_BUILD_DATE");
pub const KIND: &str = env!("OB_BUILD_KIND");

/// A script that sets `window.__OB_SHELL_BUILD__` for the shop page.
pub fn init_script() -> String {
    format!(
        "window.__OB_SHELL_BUILD__ = Object.freeze({{ version: {VERSION:?}, commit: {COMMIT:?}, dirty: {DIRTY}, buildDate: {DATE:?}, buildKind: {KIND:?} }});"
    )
}

/// Tauri's default menu, with an About panel that reads
/// "Version 0.1.0 (a1b2c3d)" and names the build kind and date.
pub fn menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    #[cfg(target_os = "macos")]
    if let Some(tauri::menu::MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() {
        use tauri::menu::{AboutMetadata, PredefinedMenuItem};
        let about = AboutMetadata {
            name: Some(app.package_info().name.clone()),
            short_version: Some(VERSION.into()),
            version: Some(format!("{COMMIT}{}", if DIRTY { "-dirty" } else { "" })),
            credits: Some(format!("{KIND} build {DATE}\nBitcoin {}", crate::node::NETWORK)),
            ..Default::default()
        };
        app_menu.remove_at(0)?;
        app_menu.insert(&PredefinedMenuItem::about(app, None, Some(about))?, 0)?;
    }
    Ok(menu)
}
