/// Desktop shell. The shop screens live in the Expo web export, not here.
///
/// Opening the app starts its own `ob-node` on a free loopback port, then opens
/// the shop window with that origin injected before the page runs. Quitting
/// the app stops the node. A second launch focuses the first window.
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
    let started = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("Could not find the app data folder: {err}"))
        .and_then(|dir| node::Sidecar::start(&dir));
    let sidecar = match started {
        Ok(sidecar) => sidecar,
        Err(message) => return fail(&app, message),
    };
    let origin = sidecar.origin.clone();
    *app.state::<NodeState>().0.lock().unwrap() = Some(sidecar);
    if let Err(err) = open_main_window(&app, &origin) {
        fail(&app, format!("Could not open the shop window: {err}"));
    }
}

fn open_main_window(app: &AppHandle, origin: &str) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
        .title("OpenBazaar")
        .inner_size(960.0, 720.0)
        .resizable(true)
        .background_color(tauri::window::Color(0x06, 0x38, 0x53, 0xff))
        .initialization_script(format!("window.__OB_NODE_ORIGIN__ = {origin:?};"))
        .build()?;
    Ok(())
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
