//! Menu-bar control for the desktop shell.
//!
//! The icon is a black-and-clear template, so macOS tints it for the menu bar
//! in light and dark mode. A left click opens the shop window the node already
//! serves. The menu (right click) can open that same window, open the LAN
//! setup page, or quit. The status line is not a command.

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, Wry};

use crate::focus_main_window;

const OPEN: &str = "open";
const PAIR: &str = "pair";
const QUIT: &str = "quit";
const PAIR_WINDOW: &str = "pair";
/// The home node's setup page, when that node was started with `--lan`.
const PAIR_URL: &str = "http://127.0.0.1:8790/";

/// Shop window background, `#063853`, shown until the setup page paints.
const NAVY: tauri::window::Color = tauri::window::Color(0x06, 0x38, 0x53, 0xff);

/// The icon stays in the menu bar for as long as this value is alive.
struct TrayState {
    #[allow(dead_code)]
    icon: TrayIcon<Wry>,
    status: MenuItem<Wry>,
}

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, OPEN, "Open OpenBazaar", true, None::<&str>)?;
    let pair = MenuItem::with_id(app, PAIR, "Pair a device", true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "Node stopped", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT, "Quit", true, None::<&str>)?;
    let above_status = PredefinedMenuItem::separator(app)?;
    let above_quit = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[&open, &pair, &above_status, &status, &above_quit, &quit],
    )?;

    let icon = TrayIconBuilder::with_id("openbazaar")
        .menu(&menu)
        .tooltip("OpenBazaar")
        .icon(Image::new_owned(template_rgba(), ICON, ICON))
        .icon_as_template(true)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            OPEN => focus_main_window(app),
            PAIR => open_pair(app),
            QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                focus_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    app.manage(TrayState { icon, status });
    eprintln!("OpenBazaar menu bar icon is up");
    Ok(())
}

pub fn set_running(app: &AppHandle, running: bool) {
    let Some(tray) = app.try_state::<TrayState>() else {
        return;
    };
    let status = tray.status.clone();
    drop(tray);
    std::thread::spawn(move || {
        let text = if running {
            "Node running"
        } else {
            "Node stopped"
        };
        let _ = status.set_text(text);
    });
}

/// The LAN setup page, in its own webview so the shop window stays put.
fn open_pair(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(PAIR_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let Ok(url) = PAIR_URL.parse() else {
        return;
    };
    let _ = WebviewWindowBuilder::new(app, PAIR_WINDOW, WebviewUrl::External(url))
        .title("Pair a device")
        .inner_size(480.0, 720.0)
        .resizable(true)
        .background_color(NAVY)
        .build();
}

const ICON: u32 = 36;

/// A storefront silhouette: awning, two windows, a door. Black pixels only,
/// which is what a macOS template image is.
fn template_rgba() -> Vec<u8> {
    let mut px = vec![0u8; (ICON * ICON * 4) as usize];
    let mut fill = |x0: i32, y0: i32, x1: i32, y1: i32, alpha: u8| {
        for y in y0..y1 {
            for x in x0..x1 {
                if (0..ICON as i32).contains(&x) && (0..ICON as i32).contains(&y) {
                    let i = ((y as u32 * ICON + x as u32) * 4) as usize;
                    px[i + 3] = alpha;
                }
            }
        }
    };
    fill(4, 5, 32, 13, 255);
    for (cx, cy) in [(9, 13), (18, 13), (27, 13)] {
        for y in cy..cy + 6 {
            for x in cx - 5..cx + 6 {
                let dx = x - cx;
                let dy = y - cy;
                if dx * dx + dy * dy <= 20 {
                    fill(x, y, x + 1, y + 1, 255);
                }
            }
        }
    }
    fill(6, 18, 30, 33, 255);
    fill(9, 21, 14, 26, 0);
    fill(22, 21, 27, 26, 0);
    fill(15, 24, 21, 33, 0);
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_bar_icon_is_a_black_template() {
        let px = template_rgba();
        assert_eq!(px.len(), (ICON * ICON * 4) as usize);
        let mut opaque = 0;
        let mut clear = 0;
        for pixel in px.chunks_exact(4) {
            assert_eq!(&pixel[..3], &[0, 0, 0], "template pixels are black");
            match pixel[3] {
                0 => clear += 1,
                255 => opaque += 1,
                other => panic!("template alpha is only clear or solid, not {other}"),
            }
        }
        assert!(opaque > 100, "the shop mark has to be visible");
        assert!(
            clear > 100,
            "the background stays clear so the menu bar shows through"
        );
        let at = |x: u32, y: u32| px[((y * ICON + x) * 4 + 3) as usize];
        assert_eq!(at(8, 8), 255, "awning");
        assert_eq!(at(18, 28), 0, "door");
        assert_eq!(at(11, 23), 0, "window");
    }
}
