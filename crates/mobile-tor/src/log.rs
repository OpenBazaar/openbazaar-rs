//! What the phone's Tor client is doing, for the device log.
//!
//! The app gives a sink: on iOS `ob_tor_set_log` (Swift writes each line with
//! NSLog, which a Release build shows in Console.app and under
//! `xcrun devicectl device process launch --console`); on Android logcat.
//! Without a sink the lines go nowhere.
//!
//! A line never carries a request or an answer, only their sizes, and an
//! onion address is shortened to `abcdef…wxyz`.

use std::sync::RwLock;

type Sink = Box<dyn Fn(&str) + Send + Sync>;

static SINK: RwLock<Option<Sink>> = RwLock::new(None);

pub fn set_sink(sink: Sink) {
    *SINK.write().unwrap_or_else(|err| err.into_inner()) = Some(sink);
}

pub(crate) fn line(text: &str) {
    if let Some(sink) = SINK.read().unwrap_or_else(|err| err.into_inner()).as_ref() {
        sink(&format!("ob-tor: {}", scrub(text)));
    }
}

/// `text` on one line of at most 300 characters, with every whole onion name
/// (56 characters) in it shortened: enough to match the node's log, too
/// little to dial.
pub(crate) fn scrub(text: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    for ch in text.chars().chain([' ']) {
        if matches!(ch, 'a'..='z' | '2'..='7') {
            run.push(ch);
            continue;
        }
        if run.len() == 56 {
            out.push_str(&format!("{}…{}", &run[..6], &run[52..]));
        } else {
            out.push_str(&run);
        }
        run.clear();
        out.push(if ch.is_control() { ' ' } else { ch });
    }
    out.trim_end().chars().take(300).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Every line logged by any test so far.
    pub(crate) fn captured() -> Vec<String> {
        set_sink(Box::new(|line| LINES.lock().unwrap().push(line.to_string())));
        LINES.lock().unwrap().clone()
    }

    #[test]
    fn a_line_is_one_line_and_never_holds_a_whole_onion_address() {
        let onion = "2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid";
        captured();
        line(&format!("dial {onion}.onion: failed after 3s: no route to\n{onion}.onion:80"));
        let lines = captured();
        let line = lines.iter().find(|line| line.contains("failed after 3s")).unwrap();
        assert_eq!(
            line,
            "ob-tor: dial 2gzyxa…3wid.onion: failed after 3s: no route to 2gzyxa…3wid.onion:80"
        );
        assert!(lines.iter().all(|line| !line.contains(onion)));
        // Shorter and longer runs are not addresses and stay as they are.
        assert_eq!(scrub("try 2 of abcdefg"), "try 2 of abcdefg");
        assert_eq!(scrub(&"a".repeat(400)).chars().count(), 300);
    }
}
