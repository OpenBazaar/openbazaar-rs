//! Update download, verification, installation, and location checking.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, Url};
use tauri_plugin_dialog::MessageDialogKind;
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::watch;

use super::status::{change, tray_progress_text, Meter, UpdaterState};
use super::{Busy, BUSY_MESSAGE};

/// Release notes longer than this are cut in the dialog.
pub(crate) const NOTES_LIMIT: usize = 700;

/// The update a check found, kept so release notes and "Download & install"
/// need no second look at `latest.json`.
pub(crate) struct PendingUpdate {
    pub(crate) update: Update,
}

/// Ask the channel's manifest, through the node's Tor. Without Tor there is
/// no check: it would tell the manifest host this machine's address.
pub(crate) async fn check(app: &AppHandle) -> Result<Option<Update>, String> {
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

/// What an install needs once it is allowed to start.
pub(crate) struct InstallJob {
    pub(crate) update: Update,
    pub(crate) cancel: watch::Receiver<bool>,
    pub(crate) _busy: Busy,
}

/// Check that an install can start, and mark it started: the cached update,
/// the cancel signal, and "downloading" on the page.
pub(crate) fn reserve_install(app: &AppHandle) -> Result<InstallJob, String> {
    let busy = Busy::take().ok_or_else(|| BUSY_MESSAGE.to_string())?;
    let update = app
        .state::<UpdaterState>()
        .lock()
        .pending
        .as_ref()
        .map(|pending| pending.update.clone())
        .ok_or_else(|| "There is no update to install. Check for updates first.".to_string())?;
    if let Some(problem) = current_location_problem() {
        change(app, |shared| shared.status.location_problem = Some(problem.clone()));
        return Err(problem);
    }
    let (sender, cancel) = watch::channel(false);
    change(app, |shared| {
        shared.status.begin_download();
        shared.cancel = Some(sender);
    });
    Ok(InstallJob { update, cancel, _busy: busy })
}

pub(crate) enum Downloaded {
    Bytes(Vec<u8>),
    Cancelled,
}

/// Download first, so the node keeps running while the bytes come in; then
/// stop the node, so its binary can be replaced, and install. `dialog`: say a
/// failure in a dialog too (the install was started from one).
pub(crate) fn install_now(app: &AppHandle, job: InstallJob, dialog: bool) {
    let InstallJob { update, cancel, _busy } = job;
    crate::tray::set_status(app, &tray_progress_text(&update.version, 0, None));
    let tracker = Tracker::new(app, &update.version);
    let work = async {
        update
            .download(|chunk, total| tracker.chunk(chunk, total), || tracker.finished())
            .await
            .map_err(|err| plain_error(&err.to_string()))
    };
    let outcome = tauri::async_runtime::block_on(race_cancel(work, cancel));
    change(app, |shared| shared.cancel = None);
    match outcome {
        Ok(Downloaded::Bytes(bytes)) => {
            change(app, |shared| shared.status.installing());
            crate::tray::set_status(app, &format!("Installing OpenBazaar {}…", update.version));
            crate::stop_node(app);
            if let Err(err) = update.install(bytes) {
                let message = plain_error(&err.to_string());
                change(app, |shared| shared.status.failed(message.clone()));
                super::tell(app, MessageDialogKind::Error, "The update did not install", &format!(
                    "{message}\n\nOpenBazaar will restart on the version you have."
                ));
            }
            // Windows: the installer has taken over and this process is exiting.
            app.restart();
        }
        Ok(Downloaded::Cancelled) => {
            change(app, |shared| shared.status.cancelled());
            crate::tray::restore_status(app);
        }
        Err(message) => {
            change(app, |shared| shared.status.failed(message.clone()));
            crate::tray::restore_status(app);
            if dialog {
                super::tell(app, MessageDialogKind::Error, "The update did not install", &message);
            }
        }
    }
}

/// Run a download against a cancel signal: whichever finishes first wins, and
/// a download that finishes after the user cancelled is thrown away.
pub(crate) async fn race_cancel<F>(work: F, cancel: watch::Receiver<bool>) -> Result<Downloaded, String>
where
    F: Future<Output = Result<Vec<u8>, String>>,
{
    let mut waiter = cancel.clone();
    tokio::select! {
        result = work => {
            let bytes = result?;
            Ok(if *cancel.borrow() { Downloaded::Cancelled } else { Downloaded::Bytes(bytes) })
        }
        _ = cancel_requested(&mut waiter) => Ok(Downloaded::Cancelled),
    }
}

async fn cancel_requested(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow_and_update() {
            return;
        }
        if cancel.changed().await.is_err() {
            // The sender is gone, so nobody can cancel any more.
            std::future::pending::<()>().await;
        }
    }
}

/// Why this copy cannot be replaced where it runs, if it cannot.
pub(crate) fn current_location_problem() -> Option<String> {
    replaced_path().and_then(|path| location_problem(&path, is_read_only(&path)))
}

#[cfg(target_os = "macos")]
pub(crate) const MOVE_HINT: &str = "Quit OpenBazaar, drag it into your Applications folder, eject the OpenBazaar disk image if it is mounted, open OpenBazaar from Applications, and check for updates again.";
#[cfg(not(target_os = "macos"))]
pub(crate) const MOVE_HINT: &str = "Quit OpenBazaar, move it to a folder you own (for example ~/Applications), start it from there, and check for updates again.";

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
pub(crate) fn is_read_only(path: &Path) -> bool {
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

/// Sends a download's progress to the shared state and the menu bar.
pub(crate) struct Tracker<'a> {
    app: &'a AppHandle,
    version: String,
    started: Instant,
    meter: Mutex<Meter>,
}

impl<'a> Tracker<'a> {
    pub(crate) fn new(app: &'a AppHandle, version: &str) -> Self {
        Tracker { app, version: version.to_string(), started: Instant::now(), meter: Mutex::new(Meter::default()) }
    }

    pub(crate) fn chunk(&self, len: usize, total: Option<u64>) {
        let due = {
            let mut meter = self.meter.lock().unwrap_or_else(|err| err.into_inner());
            meter.record(len as u64, total, self.started.elapsed())
        };
        if due {
            self.publish();
        }
    }

    /// The last bytes are in: say so now, whatever the throttle says.
    pub(crate) fn finished(&self) {
        self.publish();
    }

    fn publish(&self) {
        let (downloaded, total, speed) = {
            let meter = self.meter.lock().unwrap_or_else(|err| err.into_inner());
            (meter.downloaded, meter.total, meter.speed)
        };
        change(self.app, |shared| shared.status.progress(downloaded, total, speed));
        crate::tray::set_status(self.app, &tray_progress_text(&self.version, downloaded, total));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn race(
        work: impl Future<Output = Result<Vec<u8>, String>>,
        cancel: watch::Receiver<bool>,
    ) -> Result<Downloaded, String> {
        tauri::async_runtime::block_on(race_cancel(work, cancel))
    }

    #[test]
    fn a_download_that_finishes_gives_its_bytes() {
        let (_sender, receiver) = watch::channel(false);
        match race(async { Ok(vec![1, 2, 3]) }, receiver) {
            Ok(Downloaded::Bytes(bytes)) => assert_eq!(bytes, vec![1, 2, 3]),
            _ => panic!("expected the bytes"),
        }
    }

    #[test]
    fn a_download_that_fails_gives_its_error() {
        let (_sender, receiver) = watch::channel(false);
        match race(async { Err("boom".to_string()) }, receiver) {
            Err(message) => assert_eq!(message, "boom"),
            _ => panic!("expected the error"),
        }
    }

    #[test]
    fn cancelling_stops_a_download_that_would_never_finish() {
        let (sender, receiver) = watch::channel(false);
        // The page's Cancel arrives from another thread, a moment after the race starts.
        let page = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            sender.send(true).unwrap();
        });
        let outcome = race(std::future::pending::<Result<Vec<u8>, String>>(), receiver);
        page.join().unwrap();
        assert!(matches!(outcome, Ok(Downloaded::Cancelled)));
    }

    #[test]
    fn a_download_that_finishes_after_cancel_was_asked_is_thrown_away() {
        let (sender, receiver) = watch::channel(false);
        sender.send(true).unwrap();
        assert!(matches!(race(async { Ok(vec![9]) }, receiver), Ok(Downloaded::Cancelled)));
    }

    #[test]
    fn dropping_the_cancel_sender_does_not_cancel() {
        let (sender, receiver) = watch::channel(false);
        drop(sender);
        match race(async { Ok(vec![7]) }, receiver) {
            Ok(Downloaded::Bytes(bytes)) => assert_eq!(bytes, vec![7]),
            _ => panic!("a gone sender is not a cancel"),
        }
    }
}
