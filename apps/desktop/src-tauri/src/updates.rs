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
use std::path::{Path, PathBuf};
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
    // Say so before downloading when this copy cannot be replaced where it runs
    // (a mounted .dmg, a translocated copy): the install would fail at the end.
    if let Some(problem) = replaced_path().and_then(|path| location_problem(&path, is_read_only(&path))) {
        *LATER.lock().unwrap_or_else(|err| err.into_inner()) = Some(update.version.clone());
        tell(app, MessageDialogKind::Warning, "An OpenBazaar update is ready", &format!(
            "OpenBazaar {} is available. You have {}.\n\n{problem}",
            update.version, update.current_version
        ));
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

#[cfg(target_os = "macos")]
const MOVE_HINT: &str = "Quit OpenBazaar, drag it into your Applications folder, eject the OpenBazaar disk image if it is mounted, open OpenBazaar from Applications, and check for updates again.";
#[cfg(not(target_os = "macos"))]
const MOVE_HINT: &str = "Quit OpenBazaar, move it to a folder you own (for example ~/Applications), start it from there, and check for updates again.";

/// What an install replaces: the .app bundle on macOS, the AppImage on Linux.
/// None where an installer does the work (Windows).
#[cfg(target_os = "macos")]
fn replaced_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
}

#[cfg(target_os = "linux")]
fn replaced_path() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").map(PathBuf::from)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn replaced_path() -> Option<PathBuf> {
    None
}

/// Whether the folder holding `path` is on a read-only file system, found by
/// creating and removing a probe file. A permission error is not counted:
/// the updater may still ask for an administrator's password there.
fn is_read_only(path: &Path) -> bool {
    let Some(dir) = path.parent() else { return false };
    let probe = dir.join(format!(".openbazaar-update-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            false
        }
        Err(err) => err.raw_os_error() == Some(READ_ONLY_FS),
    }
}

/// `EROFS` on macOS and Linux.
const READ_ONLY_FS: i32 = 30;

/// Why this copy cannot be replaced where it runs, if it cannot.
pub(crate) fn location_problem(path: &Path, read_only: bool) -> Option<String> {
    let text = path.to_string_lossy();
    if text.contains("/AppTranslocation/") {
        return Some(format!(
            "macOS is running OpenBazaar from a temporary read-only copy, because it was opened where it was downloaded. {MOVE_HINT}"
        ));
    }
    if !read_only {
        return None;
    }
    if text.starts_with("/Volumes/") {
        Some(format!(
            "OpenBazaar is running from its disk image ({}), which cannot be changed. {MOVE_HINT}",
            path.display()
        ))
    } else {
        Some(format!("OpenBazaar is running from a read-only place ({}). {MOVE_HINT}", path.display()))
    }
}

/// The updater's errors name its internals; say what they mean. Callers put
/// them under a title that says what was being done.
pub(crate) fn plain_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("signature") {
        "The download is not signed by the OpenBazaar release key, so it was not installed.".into()
    } else if (lower.contains("platform") || lower.contains("target")) && lower.contains("not found") {
        "This kind of install cannot update itself (for example the Debian package). Download the new version instead.".into()
    } else if lower.contains("read-only file system") || lower.contains("os error 30") {
        format!("OpenBazaar could not replace itself because it is running from a read-only place. {MOVE_HINT}")
    } else if lower.contains("permission denied") || lower.contains("os error 13") {
        format!("OpenBazaar does not have permission to replace itself where it is installed. {MOVE_HINT}")
    } else {
        raw.to_string()
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
        assert_eq!(plain_error("connection reset"), "connection reset");
        let read_only = plain_error("Read-only file system (os error 30) at path \"/Volumes/OpenBazaar/tauri_current_app\"");
        assert!(read_only.contains("read-only place") && read_only.contains(MOVE_HINT), "{read_only}");
        assert!(plain_error("Permission denied (os error 13)").contains("permission"));
    }

    #[test]
    fn a_copy_that_cannot_be_replaced_is_named_before_any_download() {
        let dmg = Path::new("/Volumes/OpenBazaar/OpenBazaar.app");
        let said = location_problem(dmg, true).expect("a mounted disk image is refused");
        assert!(said.contains("disk image") && said.contains(MOVE_HINT), "{said}");
        assert_eq!(location_problem(dmg, false), None, "an external drive the user can write to is fine");
        let moved = Path::new("/private/var/folders/x/AppTranslocation/ABC/d/OpenBazaar.app");
        assert!(location_problem(moved, false).unwrap().contains("temporary read-only copy"));
        assert_eq!(location_problem(Path::new("/Applications/OpenBazaar.app"), false), None);
        assert!(location_problem(Path::new("/opt/OpenBazaar.AppImage"), true).unwrap().contains("read-only place"));
    }

    #[test]
    fn a_writable_folder_is_not_read_only() {
        let dir = std::env::temp_dir().join(format!("ob-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!is_read_only(&dir.join("OpenBazaar.app")));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the probe file is removed");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
