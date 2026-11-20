//! Discovery and loading of recent diagnostic files. Every problem (an
//! unreadable directory, an unreadable or unparsable file) becomes a note in
//! [`Loaded::notes`], never an error.

use std::fs;
use std::path::PathBuf;

use super::{Incident, parse_jetsam, parse_panic, parse_reset, parse_spin};
use crate::clock::parse_file_stamp;

/// Slack before `since`: panics are written after the next boot.
const STAMP_SLACK_SECS: i64 = 3600;

/// The directories macOS writes diagnostic reports to.
pub fn diag_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Library/Logs/DiagnosticReports"),
        PathBuf::from("/Library/Logs/DiagnosticReports/Retired"),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Panic,
    Jetsam,
    Spin,
    Reset,
}

/// Decides which parser (if any) handles a file, from its name alone.
pub fn classify(file_name: &str) -> Option<Kind> {
    if file_name.starts_with("panic-full-") && file_name.ends_with(".panic") {
        Some(Kind::Panic)
    } else if file_name.starts_with("JetsamEvent-") && file_name.ends_with(".ips") {
        Some(Kind::Jetsam)
    } else if file_name.ends_with(".userspace_watchdog_timeout.spin") {
        Some(Kind::Spin)
    } else if file_name.starts_with("ResetCounter-") && file_name.ends_with(".diag") {
        Some(Kind::Reset)
    } else {
        None
    }
}

#[derive(Debug, Default)]
pub struct Loaded {
    /// Sorted by time, oldest first.
    pub incidents: Vec<Incident>,
    /// Names of the files that were read.
    pub sources: Vec<String>,
    /// "could not list/read/parse …" lines for the report's Sources section.
    pub notes: Vec<String>,
}

fn parse(kind: Kind, text: &str, file_time: i64) -> Result<Incident, String> {
    match kind {
        Kind::Panic => parse_panic(text, file_time).map(Incident::Panic),
        Kind::Jetsam => parse_jetsam(text).map(Incident::Jetsam),
        Kind::Spin => parse_spin(text).map(Incident::Watchdog),
        Kind::Reset => parse_reset(text).map(Incident::Reset),
    }
}

/// Loads incidents with `time() >= since` from the direct children of `dirs`.
/// Only one file's text is held at a time.
pub fn load_incidents(dirs: &[PathBuf], since: i64) -> Loaded {
    let mut out = Loaded::default();
    for dir in dirs {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                out.notes
                    .push(format!("could not list {}: {e}", dir.display()));
                continue;
            }
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(kind) = classify(&name) else {
                continue;
            };
            let Some(stamp) = parse_file_stamp(&name) else {
                continue;
            };
            if stamp < since - STAMP_SLACK_SECS {
                continue;
            }
            let path = entry.path();
            let text = match fs::read(&path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => {
                    out.notes.push(format!("could not read {name}: {e}"));
                    continue;
                }
            };
            out.sources.push(name.clone());
            match parse(kind, &text, stamp) {
                Ok(incident) if incident.time() >= since => out.incidents.push(incident),
                Ok(_) => {}
                Err(e) => out.notes.push(format!("could not parse {name}: {e}")),
            }
        }
    }
    out.incidents.sort_by_key(Incident::time);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{Pattern, fmt_local, parse_apple_date};
    use crate::diag::fixtures;

    fn temp_dir(test: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mhm-scan-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    const DATE: &str = "2026-11-19 08:52:29.95 +0000";

    fn jetsam(date: &str) -> String {
        fixtures::jetsam_text(date, 16384, 100, 10, None, &[("a", 5, None)])
    }

    fn stamp(t: i64) -> String {
        fmt_local(t, Pattern::FileStamp)
    }

    #[test]
    fn classify_names() {
        assert_eq!(
            classify("panic-full-2026-11-19-085618.000.panic"),
            Some(Kind::Panic)
        );
        assert_eq!(
            classify("JetsamEvent-2026-11-19-085229.ips"),
            Some(Kind::Jetsam)
        );
        assert_eq!(
            classify("x_2026-11-19-085947_Host.userspace_watchdog_timeout.spin"),
            Some(Kind::Spin)
        );
        assert_eq!(
            classify("ResetCounter-2026-11-19-085750.diag"),
            Some(Kind::Reset)
        );
        assert_eq!(classify("zen_2026-11-19-105902_Host.diag"), None);
        assert_eq!(classify("ExcUserFault_Arc-2026-11-19-090135.ips"), None);
    }

    #[test]
    fn load_incidents_filters_by_stamp_and_time() {
        let dir = temp_dir("filter");
        let t = parse_apple_date(DATE).unwrap();
        fs::write(
            dir.join(format!("JetsamEvent-{}.ips", stamp(t))),
            jetsam(DATE),
        )
        .unwrap();
        let old = t - 10 * 86_400;
        fs::write(
            dir.join(format!("JetsamEvent-{}.ips", stamp(old))),
            jetsam("2026-11-09 08:52:29.95 +0000"),
        )
        .unwrap();
        let loaded = load_incidents(std::slice::from_ref(&dir), t - 7 * 86_400);
        assert_eq!(loaded.incidents.len(), 1);
        assert_eq!(loaded.sources.len(), 1);
        assert!(loaded.notes.is_empty(), "{:?}", loaded.notes);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_incidents_notes_bad_file() {
        let dir = temp_dir("bad");
        let t = parse_apple_date(DATE).unwrap();
        let name = format!("JetsamEvent-{}.ips", stamp(t));
        fs::write(dir.join(&name), "garbage").unwrap();
        let loaded = load_incidents(std::slice::from_ref(&dir), t - 3600);
        assert!(loaded.incidents.is_empty());
        assert!(
            loaded
                .notes
                .iter()
                .any(|n| n.contains("could not parse") && n.contains(&name)),
            "{:?}",
            loaded.notes
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_incidents_missing_dir_is_note() {
        let dir = std::env::temp_dir().join("mhm-scan-does-not-exist-xyz");
        let loaded = load_incidents(&[dir], 0);
        assert!(loaded.incidents.is_empty());
        assert!(loaded.notes.iter().any(|n| n.contains("could not list")));
    }
}
