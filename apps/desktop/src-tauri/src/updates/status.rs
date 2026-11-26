//! Update status, stage tracking, download progress metering, and formatting.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::watch;

use super::install::PendingUpdate;

/// The event the shop page listens to; its payload is an [`UpdateStatus`].
pub const STATUS_EVENT: &str = "ob://update-status";

/// Download progress reaches the page and the menu bar this often, at most.
pub(crate) const PUBLISH_EVERY: Duration = Duration::from_millis(250);

/// Where an update is, as the shop page and the menu bar show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Installing,
    Error,
}

/// Everything the shop page needs to draw the update banner and settings.
/// Sent whole, as JSON, by `update_status` and the `ob://update-status` event.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UpdateStatus {
    pub stage: Stage,
    pub current_version: String,
    pub available_version: Option<String>,
    pub release_notes: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub bytes_per_sec: Option<u64>,
    /// Unix seconds of the last check that reached the manifest.
    pub checked_at: Option<u64>,
    pub error: Option<String>,
    pub automatic: bool,
    /// Why this copy cannot replace itself where it runs, found before any download.
    pub location_problem: Option<String>,
    /// False in a build without an updater key, so the page can say so.
    pub can_check: bool,
}

impl UpdateStatus {
    pub(crate) fn new(current_version: String, can_check: bool, automatic: bool) -> Self {
        UpdateStatus {
            stage: Stage::Idle,
            current_version,
            available_version: None,
            release_notes: None,
            downloaded_bytes: 0,
            total_bytes: None,
            bytes_per_sec: None,
            checked_at: None,
            error: None,
            automatic,
            location_problem: None,
            can_check,
        }
    }

    pub(crate) fn reset_progress(&mut self) {
        self.downloaded_bytes = 0;
        self.total_bytes = None;
        self.bytes_per_sec = None;
    }

    pub(crate) fn forget_update(&mut self) {
        self.available_version = None;
        self.release_notes = None;
        self.location_problem = None;
        self.reset_progress();
    }

    pub(crate) fn begin_check(&mut self) {
        self.stage = Stage::Checking;
        self.error = None;
    }

    pub(crate) fn up_to_date(&mut self, now: u64) {
        self.forget_update();
        self.stage = Stage::UpToDate;
        self.error = None;
        self.checked_at = Some(now);
    }

    pub(crate) fn available(&mut self, version: &str, notes: Option<&str>, location_problem: Option<String>, now: u64) {
        self.reset_progress();
        self.stage = Stage::Available;
        self.available_version = Some(version.to_string());
        self.release_notes = notes.map(str::trim).filter(|notes| !notes.is_empty()).map(str::to_string);
        self.location_problem = location_problem;
        self.error = None;
        self.checked_at = Some(now);
    }

    /// A check that did not reach an answer. `surface`: show it as an error
    /// (the user asked); otherwise the schedule fails quietly and the page
    /// only keeps the message.
    pub(crate) fn check_failed(&mut self, message: String, surface: bool) {
        self.forget_update();
        self.stage = if surface { Stage::Error } else { Stage::Idle };
        self.error = Some(message);
    }

    pub(crate) fn begin_download(&mut self) {
        self.reset_progress();
        self.stage = Stage::Downloading;
        self.error = None;
    }

    pub(crate) fn progress(&mut self, downloaded: u64, total: Option<u64>, speed: Option<u64>) {
        self.downloaded_bytes = downloaded;
        self.total_bytes = total;
        self.bytes_per_sec = speed;
    }

    pub(crate) fn installing(&mut self) {
        self.stage = Stage::Installing;
        self.bytes_per_sec = None;
    }

    /// Back to the offer, so the user can try again without another check.
    pub(crate) fn cancelled(&mut self) {
        self.reset_progress();
        self.stage = Stage::Available;
        self.error = None;
    }

    /// A download or install that failed; the update stays known so Retry works.
    pub(crate) fn failed(&mut self, message: String) {
        self.reset_progress();
        self.stage = Stage::Error;
        self.error = Some(message);
    }
}

pub(crate) struct Shared {
    pub(crate) status: UpdateStatus,
    pub(crate) pending: Option<PendingUpdate>,
    /// Set while a download runs; sending `true` stops it.
    pub(crate) cancel: Option<watch::Sender<bool>>,
}

/// The update status and what goes with it, shared by every thread.
pub struct UpdaterState(pub(crate) Mutex<Shared>);

impl UpdaterState {
    pub fn for_app(app: &AppHandle) -> Self {
        let status = UpdateStatus::new(
            app.package_info().version.to_string(),
            super::configured(app),
            super::configured(app) && super::automatic(app),
        );
        UpdaterState(Mutex::new(Shared { status, pending: None, cancel: None }))
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.0.lock().unwrap_or_else(|err| err.into_inner())
    }
}

/// Change the shared state, then tell the shop page.
pub(crate) fn change(app: &AppHandle, edit: impl FnOnce(&mut Shared)) -> UpdateStatus {
    let state = app.state::<UpdaterState>();
    let status = {
        let mut shared = state.lock();
        edit(&mut shared);
        shared.status.clone()
    };
    if let Err(err) = app.emit(STATUS_EVENT, &status) {
        eprintln!("OpenBazaar: could not send the update status: {err}");
    }
    status
}

// How bytes are said, in the page and in the menu bar (the page has the same
// rules in `updater.ts`): binary units, one decimal, one unit for a pair.

const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];

fn unit_for(bytes: u64) -> usize {
    let mut unit = 0;
    let mut size = bytes;
    while size >= 1024 && unit < UNITS.len() - 1 {
        size /= 1024;
        unit += 1;
    }
    unit
}

fn scaled(bytes: u64, unit: usize) -> String {
    if unit == 0 {
        return bytes.to_string();
    }
    format!("{:.1}", bytes as f64 / 1024f64.powi(unit as i32))
}

/// "18.4 MB"
pub fn format_bytes(bytes: u64) -> String {
    let unit = unit_for(bytes);
    format!("{} {}", scaled(bytes, unit), UNITS[unit])
}

/// "18.4 / 43.8 MB": both in the unit of the total.
pub fn format_progress(downloaded: u64, total: u64) -> String {
    let unit = unit_for(total);
    format!("{} / {} {}", scaled(downloaded, unit), scaled(total, unit), UNITS[unit])
}

/// Whole percent, never above 100. None when the size is not known.
pub fn percent(downloaded: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|total| *total > 0)?;
    Some((u128::from(downloaded) * 100 / u128::from(total)).min(100) as u8)
}

/// The menu bar's status line while downloading.
pub fn tray_progress_text(version: &str, downloaded: u64, total: Option<u64>) -> String {
    match (percent(downloaded, total), total) {
        (Some(percent), Some(total)) => {
            format!("Downloading OpenBazaar {version}… {percent}% ({})", format_progress(downloaded, total))
        }
        _ if downloaded == 0 => format!("Downloading OpenBazaar {version}…"),
        _ => format!("Downloading OpenBazaar {version}… {}", format_bytes(downloaded)),
    }
}

/// Counts a download and decides when to say so. Pure: the caller gives the
/// time elapsed since the download began.
#[derive(Default)]
pub(crate) struct Meter {
    pub(crate) downloaded: u64,
    pub(crate) total: Option<u64>,
    pub(crate) speed: Option<u64>,
    /// Time and byte count when the speed was last measured.
    window: Option<(Duration, u64)>,
    published_at: Option<Duration>,
}

impl Meter {
    /// Count a chunk. True when this is the moment to publish: the first
    /// chunk, then once per [`PUBLISH_EVERY`].
    pub(crate) fn record(&mut self, chunk: u64, total: Option<u64>, at: Duration) -> bool {
        self.downloaded += chunk;
        if total.is_some() {
            self.total = total;
        }
        let Some(published) = self.published_at else {
            // Speed is measured from here, not from the request, so the wait
            // for the first byte over Tor does not count as slowness.
            self.window = Some((at, self.downloaded));
            self.published_at = Some(at);
            return true;
        };
        if at.saturating_sub(published) < PUBLISH_EVERY {
            return false;
        }
        if let Some((since, bytes)) = self.window {
            let seconds = at.saturating_sub(since).as_secs_f64();
            if seconds > 0.0 {
                let now = (self.downloaded - bytes) as f64 / seconds;
                let smoothed = self.speed.map_or(now, |before| before as f64 * 0.5 + now * 0.5);
                self.speed = Some(smoothed.round() as u64);
            }
        }
        self.window = Some((at, self.downloaded));
        self.published_at = Some(at);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> UpdateStatus {
        UpdateStatus::new("0.1.0".into(), true, true)
    }

    fn json(status: &UpdateStatus) -> serde_json::Value {
        serde_json::to_value(status).unwrap()
    }

    #[test]
    fn bytes_are_said_in_one_unit_with_one_decimal() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(18_400_000), "17.5 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 3), "3.0 GB");
        assert_eq!(format_bytes(1024u64.pow(4) * 2), "2048.0 GB", "nothing larger than GB");
    }

    #[test]
    fn a_pair_of_sizes_shares_the_unit_of_the_total() {
        let mb = 1024 * 1024;
        assert_eq!(format_progress(18 * mb + mb * 4 / 10, 43 * mb + mb * 8 / 10), "18.4 / 43.8 MB");
        assert_eq!(format_progress(900 * 1024, 2 * mb), "0.9 / 2.0 MB", "a part of a MB is not shown in KB");
        assert_eq!(format_progress(0, 512), "0 / 512 B");
    }

    #[test]
    fn percent_is_whole_and_capped() {
        assert_eq!(percent(0, Some(200)), Some(0));
        assert_eq!(percent(84, Some(200)), Some(42));
        assert_eq!(percent(199, Some(200)), Some(99), "rounds down, never claims done early");
        assert_eq!(percent(250, Some(200)), Some(100), "a server that sends more is still 100");
        assert_eq!(percent(10, Some(0)), None, "an empty total is not a size");
        assert_eq!(percent(10, None), None);
        assert_eq!(percent(u64::MAX, Some(u64::MAX)), Some(100), "no overflow");
    }

    #[test]
    fn the_menu_bar_line_shows_progress_with_and_without_a_total() {
        let mb = 1024 * 1024;
        assert_eq!(
            tray_progress_text("0.2.0", 18 * mb + mb * 4 / 10, Some(43 * mb + mb * 8 / 10)),
            "Downloading OpenBazaar 0.2.0… 42% (18.4 / 43.8 MB)"
        );
        assert_eq!(tray_progress_text("0.2.0", 18 * mb + mb * 4 / 10, None), "Downloading OpenBazaar 0.2.0… 18.4 MB");
        assert_eq!(tray_progress_text("0.2.0", 0, None), "Downloading OpenBazaar 0.2.0…");
        assert_eq!(tray_progress_text("0.2.0", 0, Some(mb)), "Downloading OpenBazaar 0.2.0… 0% (0.0 / 1.0 MB)");
        assert_eq!(tray_progress_text("0.2.0", 5, Some(0)), "Downloading OpenBazaar 0.2.0… 5 B", "a zero total counts as unknown");
    }

    #[test]
    fn a_new_status_serializes_with_every_field_the_page_reads() {
        let value = json(&status());
        assert_eq!(
            value,
            serde_json::json!({
                "stage": "idle",
                "current_version": "0.1.0",
                "available_version": null,
                "release_notes": null,
                "downloaded_bytes": 0,
                "total_bytes": null,
                "bytes_per_sec": null,
                "checked_at": null,
                "error": null,
                "automatic": true,
                "location_problem": null,
                "can_check": true
            })
        );
    }

    #[test]
    fn stages_serialize_as_the_words_the_page_matches_on() {
        for (stage, word) in [
            (Stage::Idle, "idle"),
            (Stage::Checking, "checking"),
            (Stage::UpToDate, "up_to_date"),
            (Stage::Available, "available"),
            (Stage::Downloading, "downloading"),
            (Stage::Installing, "installing"),
            (Stage::Error, "error"),
        ] {
            assert_eq!(serde_json::to_value(stage).unwrap(), word);
        }
    }

    #[test]
    fn a_check_that_finds_an_update_then_a_download_runs_the_stages_in_order() {
        let mut s = status();
        s.begin_check();
        assert_eq!(s.stage, Stage::Checking);
        s.available("0.2.0", Some("  Faster.  "), None, 1_700_000_000);
        assert_eq!(s.stage, Stage::Available);
        assert_eq!(s.available_version.as_deref(), Some("0.2.0"));
        assert_eq!(s.release_notes.as_deref(), Some("Faster."), "notes are trimmed");
        assert_eq!(s.checked_at, Some(1_700_000_000));
        s.begin_download();
        assert_eq!(s.stage, Stage::Downloading);
        s.progress(100, Some(400), Some(50));
        assert_eq!((s.downloaded_bytes, s.total_bytes, s.bytes_per_sec), (100, Some(400), Some(50)));
        s.installing();
        assert_eq!(s.stage, Stage::Installing);
        assert_eq!(s.bytes_per_sec, None, "no speed to show while installing");
        assert_eq!(s.available_version.as_deref(), Some("0.2.0"));
    }

    #[test]
    fn empty_notes_are_no_notes() {
        let mut s = status();
        s.available("0.2.0", Some("  \n "), None, 1);
        assert_eq!(s.release_notes, None);
    }

    #[test]
    fn cancelling_goes_back_to_the_offer_and_keeps_the_update() {
        let mut s = status();
        s.available("0.2.0", Some("Faster."), None, 1);
        s.begin_download();
        s.progress(100, Some(400), Some(50));
        s.cancelled();
        assert_eq!(s.stage, Stage::Available);
        assert_eq!((s.downloaded_bytes, s.total_bytes, s.bytes_per_sec), (0, None, None));
        assert_eq!(s.available_version.as_deref(), Some("0.2.0"));
        assert_eq!(s.release_notes.as_deref(), Some("Faster."));
        assert_eq!(s.error, None);
    }

    #[test]
    fn a_failed_download_keeps_the_update_so_retry_needs_no_check() {
        let mut s = status();
        s.available("0.2.0", None, None, 1);
        s.begin_download();
        s.progress(100, Some(400), Some(50));
        s.failed("connection reset".into());
        assert_eq!(s.stage, Stage::Error);
        assert_eq!(s.error.as_deref(), Some("connection reset"));
        assert_eq!(s.available_version.as_deref(), Some("0.2.0"), "Retry downloads this version");
        assert_eq!(s.downloaded_bytes, 0);
        s.begin_download();
        assert_eq!(s.stage, Stage::Downloading);
        assert_eq!(s.error, None, "a new attempt clears the old error");
    }

    #[test]
    fn a_check_that_fails_forgets_the_update_and_is_shown_only_when_asked_for() {
        let mut asked = status();
        asked.available("0.2.0", Some("Faster."), None, 1);
        asked.begin_check();
        asked.check_failed("no Tor".into(), true);
        assert_eq!(asked.stage, Stage::Error);
        assert_eq!(asked.available_version, None, "an error with no version means Retry checks again");
        assert_eq!(asked.release_notes, None);

        let mut scheduled = status();
        scheduled.begin_check();
        scheduled.check_failed("no Tor".into(), false);
        assert_eq!(scheduled.stage, Stage::Idle, "the schedule fails quietly");
        assert_eq!(scheduled.error.as_deref(), Some("no Tor"));
    }

    #[test]
    fn being_up_to_date_records_when_and_clears_the_offer() {
        let mut s = status();
        s.available("0.2.0", Some("Faster."), Some("read-only".into()), 1);
        s.begin_check();
        s.up_to_date(42);
        assert_eq!(s.stage, Stage::UpToDate);
        assert_eq!(s.checked_at, Some(42));
        assert_eq!((s.available_version, s.release_notes, s.location_problem), (None, None, None));
    }

    #[test]
    fn a_location_problem_travels_with_the_offer() {
        let mut s = status();
        s.available("0.2.0", None, Some("Move the app first.".into()), 1);
        assert_eq!(json(&s)["location_problem"], "Move the app first.");
        assert_eq!(s.stage, Stage::Available);
    }

    #[test]
    fn the_meter_publishes_the_first_chunk_then_at_most_four_times_a_second() {
        let ms = Duration::from_millis;
        let mut meter = Meter::default();
        assert!(meter.record(1000, Some(10_000), ms(900)), "the first chunk is published at once");
        assert!(!meter.record(1000, None, ms(1000)));
        assert!(!meter.record(1000, None, ms(1100)));
        assert!(meter.record(1000, None, ms(1150)), "250 ms after the last publish");
        assert!(!meter.record(1000, None, ms(1200)));
        assert_eq!(meter.downloaded, 5000);
        assert_eq!(meter.total, Some(10_000), "a chunk without a total keeps the one seen");
    }

    #[test]
    fn the_meter_measures_speed_from_the_first_byte_and_smooths_it() {
        let ms = Duration::from_millis;
        let mut meter = Meter::default();
        // The first byte took a second to arrive over Tor; that is not the speed.
        meter.record(1000, Some(100_000), ms(1000));
        assert_eq!(meter.speed, None, "nothing to measure yet");
        // 10 000 bytes in the next 500 ms: 20 000 B/s.
        meter.record(10_000, None, ms(1500));
        assert_eq!(meter.speed, Some(20_000));
        // 5 000 bytes in the next 500 ms: 10 000 B/s now, smoothed with the 20 000 before.
        meter.record(5_000, None, ms(2000));
        assert_eq!(meter.speed, Some(15_000));
    }
}
