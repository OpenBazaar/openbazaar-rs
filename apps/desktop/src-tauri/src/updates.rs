//! In-app updates for the desktop app. See docs/updates.md.
//!
//! The app reads a signed `latest.json` for its channel, through the node's
//! built-in Tor, a minute after launch and every 12 hours after that. When a
//! newer version is listed it asks before doing anything; on yes it downloads
//! the installer for this platform, checks its signature against the public
//! key in tauri.conf.json, stops the node, installs, and restarts.
//!
//! All of this runs in the shell, never in the shop page: the page is served
//! by the node over loopback and gets no way to start an install.
//!
//! Automatic checks can be turned off from the menu bar; a file named
//! `updates-off` in the app data folder records that. "Check for updates…"
//! always works. A version the user answered "Later" to is not offered again
//! by the automatic check until the app restarts.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Url};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::{Update, UpdaterExt};

const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(12 * 60 * 60);
const AUTO_OFF_FILE: &str = "updates-off";
/// A build whose config still carries this has no updater key and never checks.
const PLACEHOLDER_KEY: &str = "UPDATER_PUBLIC_KEY_NOT_SET";
const RELEASES_PAGE: &str = "https://github.com/OpenBazaar/openbazaar-rs/releases";
/// Release notes longer than this are cut in the dialog.
const NOTES_LIMIT: usize = 700;

/// One check or install at a time, whatever started it.
static BUSY: AtomicBool = AtomicBool::new(false);
/// The version the user put off this session.
static LATER: Mutex<Option<String>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trigger {
    Automatic,
    Asked,
}

/// Whether this build was given an updater public key.
pub fn configured(app: &AppHandle) -> bool {
    let key = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(|key| key.as_str())
        .unwrap_or("");
    key_is_set(key)
}

pub(crate) fn key_is_set(key: &str) -> bool {
    let key = key.trim();
    !key.is_empty() && key != PLACEHOLDER_KEY
}

pub fn automatic(app: &AppHandle) -> bool {
    match crate::app_data_dir(app) {
        Ok(dir) => !dir.join(AUTO_OFF_FILE).exists(),
        Err(_) => true,
    }
}

pub fn set_automatic(app: &AppHandle, on: bool) -> Result<(), String> {
    let path = crate::app_data_dir(app)?.join(AUTO_OFF_FILE);
    let result = if on {
        match std::fs::remove_file(&path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    } else {
        std::fs::write(&path, b"")
    };
    result.map_err(|err| format!("Could not save the update setting: {err}"))
}

/// Start the background schedule. Does nothing in a build without a key.
pub fn start_schedule(app: AppHandle) {
    if !configured(&app) {
        eprintln!("OpenBazaar: this build has no updater key; update checks are off");
        return;
    }
    thread::spawn(move || {
        thread::sleep(FIRST_CHECK);
        loop {
            if automatic(&app) {
                run(&app, Trigger::Automatic);
            }
            thread::sleep(CHECK_EVERY);
        }
    });
}

/// "Check for updates…" from the menu bar.
pub fn check_now(app: &AppHandle) {
    let app = app.clone();
    thread::spawn(move || {
        if !configured(&app) {
            tell(&app, MessageDialogKind::Info, "Updates", &format!(
                "This build of OpenBazaar cannot update itself. Download new versions from {RELEASES_PAGE}."
            ));
            return;
        }
        run(&app, Trigger::Asked);
    });
}

fn run(app: &AppHandle, trigger: Trigger) {
    if BUSY.swap(true, Ordering::SeqCst) {
        if trigger == Trigger::Asked {
            tell(app, MessageDialogKind::Info, "Updates", "OpenBazaar is already checking for an update.");
        }
        return;
    }
    let outcome = tauri::async_runtime::block_on(check(app));
    match outcome {
        Ok(Some(update)) => offer(app, trigger, update),
        Ok(None) => {
            if trigger == Trigger::Asked {
                tell(app, MessageDialogKind::Info, "Updates", &format!(
                    "OpenBazaar {} is the newest version.",
                    app.package_info().version
                ));
            }
        }
        Err(message) => {
            eprintln!("OpenBazaar: update check failed: {message}");
            if trigger == Trigger::Asked {
                tell(app, MessageDialogKind::Warning, "Could not check for updates", &format!(
                    "{message}\n\nNew versions are also listed at {RELEASES_PAGE}."
                ));
            }
        }
    }
    BUSY.store(false, Ordering::SeqCst);
}

/// Ask the channel's manifest, through the node's Tor. Without Tor there is
/// no check: it would tell the manifest host this machine's address.
async fn check(app: &AppHandle) -> Result<Option<Update>, String> {
    let proxy = crate::tor_proxy(app)
        .ok_or_else(|| "The node's Tor is not running yet, and updates are only checked over Tor. Try again in a minute.".to_string())?;
    let proxy: Url = proxy.parse().map_err(|_| "The node's Tor address is not valid.".to_string())?;
    let updater = app
        .updater_builder()
        .proxy(proxy)
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|err| err.to_string())?;
    updater.check().await.map_err(|err| plain_error(&err.to_string()))
}

fn offer(app: &AppHandle, trigger: Trigger, update: Update) {
    let later = LATER.lock().unwrap_or_else(|err| err.into_inner()).clone();
    if trigger == Trigger::Automatic && later.as_deref() == Some(update.version.as_str()) {
        return;
    }
    let message = offer_text(&update.current_version, &update.version, update.body.as_deref());
    let install = app
        .dialog()
        .message(message)
        .title("An OpenBazaar update is ready")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom("Install and restart".into(), "Later".into()))
        .blocking_show();
    if !install {
        *LATER.lock().unwrap_or_else(|err| err.into_inner()) = Some(update.version.clone());
        return;
    }
    if let Err(message) = install_now(app, &update) {
        tell(app, MessageDialogKind::Error, "The update did not install", &format!(
            "{message}\n\nOpenBazaar will restart on the version you have."
        ));
        app.restart();
    }
}

pub(crate) fn offer_text(current: &str, next: &str, notes: Option<&str>) -> String {
    let mut text = format!("OpenBazaar {next} is available. You have {current}.");
    if let Some(notes) = notes.map(str::trim).filter(|notes| !notes.is_empty()) {
        let cut: String = notes.chars().take(NOTES_LIMIT).collect();
        text.push_str("\n\n");
        text.push_str(&cut);
        if cut.len() < notes.len() {
            text.push('…');
        }
    }
    text.push_str(
        "\n\nThe download is checked against the OpenBazaar release key before it installs. \
         Installing stops the node and restarts the app; your wallet, store, orders, and escrows are kept.",
    );
    text
}

/// Download first, so the node keeps running while the bytes come in; then
/// stop the node, so its binary can be replaced, and install.
fn install_now(app: &AppHandle, update: &Update) -> Result<(), String> {
    crate::tray::set_status(app, &format!("Downloading OpenBazaar {}…", update.version));
    let bytes = tauri::async_runtime::block_on(update.download(|_, _| {}, || {}))
        .map_err(|err| plain_error(&err.to_string()))?;
    crate::tray::set_status(app, &format!("Installing OpenBazaar {}…", update.version));
    crate::stop_node(app);
    update.install(bytes).map_err(|err| plain_error(&err.to_string()))?;
    // Windows: the installer has taken over and this process is exiting.
    app.restart();
}

/// The updater's errors name its internals; say what they mean.
pub(crate) fn plain_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("signature") {
        "The download is not signed by the OpenBazaar release key, so it was not installed.".into()
    } else if (lower.contains("platform") || lower.contains("target")) && lower.contains("not found") {
        "This kind of install cannot update itself (for example the Debian package). Download the new version instead.".into()
    } else {
        format!("The update could not be read: {raw}")
    }
}

fn tell(app: &AppHandle, kind: MessageDialogKind, title: &str, message: &str) {
    app.dialog().message(message).title(title).kind(kind).blocking_show();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_without_a_key_does_not_check() {
        assert!(!key_is_set(""));
        assert!(!key_is_set(PLACEHOLDER_KEY));
        assert!(key_is_set("dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWdu"));
    }

    #[test]
    fn the_offer_names_both_versions_and_cuts_long_notes() {
        let notes = "x".repeat(NOTES_LIMIT + 50);
        let text = offer_text("0.1.0-alpha.14", "0.1.0-alpha.15", Some(&notes));
        assert!(text.starts_with("OpenBazaar 0.1.0-alpha.15 is available. You have 0.1.0-alpha.14."));
        assert!(text.contains('…'));
        assert!(!text.contains(&"x".repeat(NOTES_LIMIT + 1)));
        assert!(text.contains("release key"));
    }

    #[test]
    fn updater_errors_are_put_plainly() {
        assert!(plain_error("Signature verification failed").contains("not signed"));
        assert!(plain_error("the platform `linux-x86_64-deb` was not found on the response `platforms` object").contains("cannot update itself"));
        assert!(plain_error("connection reset").contains("connection reset"));
    }
}
