/// Desktop shell. The shop screens live in the Expo web export, not here.
///
/// Opening the app starts its own `ob-node` on a loopback port it keeps across
/// launches, with `--local-shop` pointing at the bundled web export, then opens
/// the shop window on that loopback origin. The page is the node's local shop
/// (`ob-local-shop`), so it offers Continue on this computer instead of a pairing
/// code; the pairing token stays in this process. A menu-bar icon stays up with
/// the process: choosing Open shows this same shop window, Open in browser signs
/// a browser on this machine into that same shop, and Quit stops the node.
/// Closing the shop window hides it and leaves the node running. A second launch
/// focuses the first window.
///
/// `OB_DESKTOP_DATA_DIR=<absolute path>` runs a separate, disposable instance
/// instead: its own data folder and webview storage, its node's keys in a file
/// vault rather than the OS keychain, and no hand-off to an app already open.
mod build;
mod node;
mod tray;

use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

const MAIN_WINDOW: &str = "main";

struct NodeState(Mutex<Option<node::Sidecar>>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();
    if separate_profile().is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            focus_main_window(app);
        }));
    }
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .manage(NodeState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![home_node_mode, set_home_node_mode])
        .menu(build::menu)
        .on_window_event(|window, event| {
            if window.label() == MAIN_WINDOW {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            tray::install(app.handle())?;
            let handle = app.handle().clone();
            std::thread::spawn(move || start(handle));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building OpenBazaar desktop");

    app.run(|app, event| match event {
        RunEvent::Exit => stop_node(app),
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => focus_main_window(app),
        _ => {}
    });
}

fn separate_profile() -> Option<PathBuf> {
    std::env::var_os("OB_DESKTOP_DATA_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
}

fn start(app: AppHandle) {
    let profile = separate_profile();
    let Some(shop) = shop_dir(&app) else {
        return fail(&app, "The shop files are missing from this app.".into());
    };
    let started = app_data_dir(&app)
        .and_then(|dir| node::Sidecar::start(&dir, None, profile.is_some(), Some(&shop)));
    let sidecar = match started {
        Ok(sidecar) => sidecar,
        Err(message) => return fail(&app, message),
    };
    let origin = sidecar.origin.clone();
    *app.state::<NodeState>().0.lock().unwrap() = Some(sidecar);
    tray::set_running(&app, true, Some(&origin));
    if let Err(err) = open_main_window(&app, &origin, profile.as_deref()) {
        fail(&app, format!("Could not open the shop window: {err}"));
    }
}

fn open_main_window(
    app: &AppHandle,
    origin: &str,
    profile: Option<&std::path::Path>,
) -> Result<(), String> {
    let url: tauri::Url = node::shop_page_url(origin)?
        .parse()
        .map_err(|_| "The shop address is not on this machine.".to_string())?;
    let mut window = WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::External(url));
    if let Some(dir) = profile {
        window = separate_storage(window, dir);
    }
    window
        .title("OpenBazaar")
        .inner_size(960.0, 720.0)
        .resizable(true)
        .background_color(tauri::window::Color(0x0b, 0x17, 0x24, 0xff))
        .initialization_script(format!(
            "window.__OB_NODE_ORIGIN__ = {origin:?}; {}",
            build::init_script()
        ))
        .build()
        .map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
fn home_node_mode(app: AppHandle) -> Result<bool, String> {
    Ok(node::home_mode(&app_data_dir(&app)?))
}

/// Save the choice and restart the node on the same port, so the window keeps
/// its loopback origin. Remote access runs only while the app is open.
#[tauri::command]
fn set_home_node_mode(app: AppHandle, on: bool) -> Result<bool, String> {
    let dir = app_data_dir(&app)?;
    node::set_home_mode(&dir, on).map_err(|err| format!("Could not save the setting: {err}"))?;
    let state = app.state::<NodeState>();
    let mut slot = state.0.lock().unwrap();
    let port = slot.as_ref().map(|sidecar| sidecar.port);
    tray::set_running(&app, false, None);
    if let Some(mut old) = slot.take() {
        old.stop();
    }
    let shop = shop_dir(&app).ok_or_else(|| "The shop files are missing from this app.".to_string())?;
    let started = node::Sidecar::start(&dir, port, separate_profile().is_some(), Some(&shop))?;
    let origin = started.origin.clone();
    *slot = Some(started);
    tray::set_running(&app, true, Some(&origin));
    Ok(on)
}

fn app_data_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    if let Some(dir) = separate_profile() {
        return Ok(dir);
    }
    app.path()
        .app_data_dir()
        .map_err(|err| format!("Could not find the app data folder: {err}"))
}

/// Webview storage of its own, so a separate profile shares no saved cart or settings.
#[cfg(target_os = "macos")]
fn separate_storage<'a>(
    window: WebviewWindowBuilder<'a, tauri::Wry, AppHandle>,
    dir: &std::path::Path,
) -> WebviewWindowBuilder<'a, tauri::Wry, AppHandle> {
    use std::hash::{Hash, Hasher};
    let half = |salt: u8| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (salt, dir).hash(&mut hasher);
        hasher.finish().to_le_bytes()
    };
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&half(0));
    id[8..].copy_from_slice(&half(1));
    window.data_store_identifier(id)
}

#[cfg(not(target_os = "macos"))]
fn separate_storage<'a>(
    window: WebviewWindowBuilder<'a, tauri::Wry, AppHandle>,
    dir: &std::path::Path,
) -> WebviewWindowBuilder<'a, tauri::Wry, AppHandle> {
    window.data_directory(dir.join("webview"))
}

pub(crate) fn browser_session(app: &AppHandle) -> Option<(String, String)> {
    let state = app.state::<NodeState>();
    let guard = state.0.lock().ok()?;
    let node = guard.as_ref()?;
    Some((node.origin.clone(), node.token.clone()))
}

fn shop_dir(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(resources) = app.path().resource_dir() {
        let bundled = resources.join("shop");
        if bundled.join("index.html").is_file() {
            return Some(bundled);
        }
    }
    node::shop_directory()
}

pub(crate) fn focus_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn stop_node(app: &AppHandle) {
    if let Some(mut sidecar) = app.state::<NodeState>().0.lock().unwrap().take() {
        sidecar.stop();
    }
}

fn fail(app: &AppHandle, message: String) {
    stop_node(app);
    tray::set_running(app, false, None);
    app.dialog()
        .message(message)
        .title("OpenBazaar could not start")
        .kind(MessageDialogKind::Error)
        .blocking_show();
    app.exit(1);
}
