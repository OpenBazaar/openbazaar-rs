//! In-app updates for the desktop app. See docs/updates.md.
//!
//! The app reads a signed `latest.json` for its channel, through the node's
//! built-in Tor, a minute after launch and every 12 hours after that. When a
//! newer version is listed it asks before doing anything; on yes it downloads
//! the installer for this platform, checks its signature against the public
//! key in tauri.conf.json, stops the node, installs, and restarts.
//!
//! One [`UpdateStatus`] is shared by every thread. The shop page reads it
//! (`update_status`), hears of every change (`ob://update-status`), and drives
//! the update through commands: check, download and install, cancel. The menu
//! bar's status line follows the download. With no shop window open, the
//! questions are asked in dialogs, as before.
//!
//! All of this runs in the shell, never in the shop page: the page is served
//! by the node over loopback, and can only ask the shell to install the one
//! update the shell itself found, signed by the release key. It never names
//! a URL, a file, or bytes.
//!
//! Automatic checks can be turned off from the menu bar; a file named
//! `updates-off` in the app data folder records that. "Check for updates…"
//! always works. A version the user answered "Later" to is not offered again
//! by the automatic check until the app restarts.

pub mod install;
pub mod status;

#[allow(unused_imports)]
pub use status::{
    format_bytes, format_progress, percent, tray_progress_text, Stage, UpdateStatus, UpdaterState,
    STATUS_EVENT,
};
pub(crate) use install::PendingUpdate;
pub(crate) use status::change;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::Update;

const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(12 * 60 * 60);
const AUTO_OFF_FILE: &str = "updates-off";
/// A build whose config still carries this has no updater key and never checks.
const PLACEHOLDER_KEY: &str = "UPDATER_PUBLIC_KEY_NOT_SET";
const RELEASES_PAGE: &str = "https://github.com/OpenBazaar/openbazaar-rs/releases";
pub(crate) const BUSY_MESSAGE: &str = "OpenBazaar is already checking for or installing an update.";

/// One check or install at a time, whatever started it.
static BUSY: AtomicBool = AtomicBool::new(false);
/// The version the user put off this session.
static LATER: Mutex<Option<String>> = Mutex::new(None);

/// Holds [`BUSY`] until dropped.
pub(crate) struct Busy;

impl Busy {
    pub(crate) fn take() -> Option<Busy> {
        (!BUSY.swap(true, Ordering::SeqCst)).then_some(Busy)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// The schedule: asks in a dialog only when no window is open.
    Automatic,
    /// The menu bar or the app menu: says the outcome in a dialog.
    Asked,
    /// The shop page: the outcome is only the shared status, shown on the page.
    Ui,
}

/// The current status, without sending an event (the page polls this).
pub fn snapshot(app: &AppHandle) -> UpdateStatus {
    let state = app.state::<UpdaterState>();
    let mut shared = state.lock();
    shared.status.automatic = shared.status.can_check && automatic(app);
    shared.status.clone()
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|since| since.as_secs()).unwrap_or(0)
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

/// Save the choice, then show it everywhere: the page, and the menu bar's check box.
pub fn apply_automatic(app: &AppHandle, on: bool) -> Result<UpdateStatus, String> {
    set_automatic(app, on)?;
    let status = change(app, |shared| shared.status.automatic = shared.status.can_check && automatic(app));
    crate::tray::sync_auto_updates(app);
    Ok(status)
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

/// "Check for updates" on the shop page. Returns at once, already "checking",
/// so the page never waits on Tor; the answer arrives as the shared status.
pub fn start_check(app: &AppHandle) -> UpdateStatus {
    if !configured(app) {
        return snapshot(app);
    }
    let Some(busy) = Busy::take() else {
        return snapshot(app);
    };
    let status = change(app, |shared| shared.status.begin_check());
    let app = app.clone();
    thread::spawn(move || check_and_report(&app, Trigger::Ui, busy));
    status
}

fn run(app: &AppHandle, trigger: Trigger) {
    let Some(busy) = Busy::take() else {
        if trigger == Trigger::Asked {
            tell(app, MessageDialogKind::Info, "Updates", "OpenBazaar is already checking for an update.");
        }
        return;
    };
    change(app, |shared| shared.status.begin_check());
    check_and_report(app, trigger, busy);
}

/// Ask the manifest and record the answer. Releases the busy flag before any
/// dialog, so the page can start the install while a dialog is open.
fn check_and_report(app: &AppHandle, trigger: Trigger, busy: Busy) {
    let outcome = tauri::async_runtime::block_on(install::check(app));
    let now = unix_now();
    match outcome {
        Ok(Some(update)) => {
            let problem = install::current_location_problem();
            change(app, |shared| {
                shared.status.available(&update.version, update.body.as_deref(), problem, now);
                shared.pending = Some(PendingUpdate { update: update.clone() });
            });
            drop(busy);
            announce(app, trigger, &update);
        }
        Ok(None) => {
            change(app, |shared| {
                shared.status.up_to_date(now);
                shared.pending = None;
            });
            drop(busy);
            if trigger == Trigger::Asked {
                tell(app, MessageDialogKind::Info, "Updates", &format!(
                    "OpenBazaar {} is the newest version.",
                    app.package_info().version
                ));
            }
        }
        Err(message) => {
            eprintln!("OpenBazaar: update check failed: {message}");
            change(app, |shared| {
                shared.status.check_failed(message.clone(), trigger != Trigger::Automatic);
                shared.pending = None;
            });
            drop(busy);
            if trigger == Trigger::Asked {
                tell(app, MessageDialogKind::Warning, "Could not check for updates", &format!(
                    "{message}\n\nNew versions are also listed at {RELEASES_PAGE}."
                ));
            }
        }
    }
}

fn main_window_visible(app: &AppHandle) -> bool {
    app.get_webview_window(crate::MAIN_WINDOW)
        .is_some_and(|window| window.is_visible().unwrap_or(false))
}

/// An update was found. With the shop window open, its banner is the offer;
/// without one, ask in a dialog, as the app always has.
fn announce(app: &AppHandle, trigger: Trigger, update: &Update) {
    if trigger == Trigger::Ui {
        return;
    }
    if main_window_visible(app) {
        if trigger == Trigger::Asked {
            crate::focus_main_window(app);
        }
        return;
    }
    let later = LATER.lock().unwrap_or_else(|err| err.into_inner()).clone();
    if trigger == Trigger::Automatic && later.as_deref() == Some(update.version.as_str()) {
        return;
    }
    // Say so before downloading when this copy cannot be replaced where it runs
    // (a mounted .dmg, a translocated copy): the install would fail at the end.
    if let Some(problem) = snapshot(app).location_problem {
        *LATER.lock().unwrap_or_else(|err| err.into_inner()) = Some(update.version.clone());
        tell(app, MessageDialogKind::Warning, "An OpenBazaar update is ready", &format!(
            "OpenBazaar {} is available. You have {}.\n\n{problem}",
            update.version, update.current_version
        ));
        return;
    }
    let message = install::offer_text(&update.current_version, &update.version, update.body.as_deref());
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
    match install::reserve_install(app) {
        Ok(job) => install::install_now(app, job, true),
        Err(message) => tell(app, MessageDialogKind::Error, "The update did not install", &message),
    }
}

/// Download and install the cached update, on a thread of its own so the page
/// sees "downloading" at once. Returns that status.
pub fn install_available(app: &AppHandle) -> Result<UpdateStatus, String> {
    let job = install::reserve_install(app)?;
    let status = snapshot(app);
    crate::tray::set_status(app, &tray_progress_text(&job.update.version, 0, None));
    let app = app.clone();
    thread::spawn(move || install::install_now(&app, job, false));
    Ok(status)
}

/// Stop the download in flight, if there is one. The thread running it puts
/// the page back on the offer.
pub fn cancel_download(app: &AppHandle) -> UpdateStatus {
    if let Some(cancel) = app.state::<UpdaterState>().lock().cancel.as_ref() {
        let _ = cancel.send(true);
    }
    snapshot(app)
}

pub(crate) fn tell(app: &AppHandle, kind: MessageDialogKind, title: &str, message: &str) {
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
}
