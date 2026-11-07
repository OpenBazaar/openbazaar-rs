/// Desktop shell. The shop screens live in the Expo web export, not here.
///
/// Opening the app starts its own `ob-node` on a free loopback port, then opens
/// the shop window with that origin and the node's pairing token injected
/// before the page runs. The token is set only on the app's own origin, so a
/// page the webview might navigate to never sees it, and the node itself
/// gives the token to no request. Quitting
/// the app stops the node. A second launch focuses the first window.
mod build;
mod node;

use std::sync::Mutex;

use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

const MAIN_WINDOW: &str = "main";

struct NodeState(Mutex<Option<node::Sidecar>>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            focus_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(NodeState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![home_node_mode, set_home_node_mode])
        .menu(build::menu)
        .setup(|app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || start(handle));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building OpenBazaar desktop");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            stop_node(app);
        }
    });
}

fn start(app: AppHandle) {
    let started = app_data_dir(&app).and_then(|dir| node::Sidecar::start(&dir, None));
    let sidecar = match started {
        Ok(sidecar) => sidecar,
        Err(message) => return fail(&app, message),
    };
    let origin = sidecar.origin.clone();
    let token = sidecar.token.clone();
    *app.state::<NodeState>().0.lock().unwrap() = Some(sidecar);
    if let Err(err) = open_main_window(&app, &origin, &token) {
        fail(&app, format!("Could not open the shop window: {err}"));
    }
}

const APP_ORIGINS: &str = r#"["tauri://localhost","http://tauri.localhost","https://tauri.localhost"]"#;

fn open_main_window(app: &AppHandle, origin: &str, token: &str) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
        .title("OpenBazaar")
        .inner_size(960.0, 720.0)
        .resizable(true)
        .background_color(tauri::window::Color(0x06, 0x38, 0x53, 0xff))
        .initialization_script(format!(
            "window.__OB_NODE_ORIGIN__ = {origin:?}; if ({APP_ORIGINS}.includes(window.location.origin)) {{ window.__OB_PAIR_TOKEN__ = {token:?}; {} }}",
            build::init_script()
        ))
        .build()?;
    Ok(())
}

#[tauri::command]
fn home_node_mode(app: AppHandle) -> Result<bool, String> {
    Ok(node::home_mode(&app_data_dir(&app)?))
}

/// Save the choice and restart the node on the same port, so the window keeps
/// its origin and token. Remote access runs only while the app is open.
#[tauri::command]
fn set_home_node_mode(app: AppHandle, on: bool) -> Result<bool, String> {
    let dir = app_data_dir(&app)?;
    node::set_home_mode(&dir, on).map_err(|err| format!("Could not save the setting: {err}"))?;
    let state = app.state::<NodeState>();
    let mut slot = state.0.lock().unwrap();
    let port = slot.as_ref().map(|sidecar| sidecar.port);
    if let Some(mut old) = slot.take() {
        old.stop();
    }
    *slot = Some(node::Sidecar::start(&dir, port)?);
    Ok(on)
}

fn app_data_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|err| format!("Could not find the app data folder: {err}"))
}

fn focus_main_window(app: &AppHandle) {
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
    app.dialog()
        .message(message)
        .title("OpenBazaar could not start")
        .kind(MessageDialogKind::Error)
        .blocking_show();
    app.exit(1);
}
