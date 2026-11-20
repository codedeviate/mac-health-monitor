//! Health report: everything gathered for one report, and the pure logic built on it.

pub mod findings;
pub mod render;

use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::clock::{Pattern, boot_time, fmt_local, fmt_relative, now_unix};
use crate::config::Config;
use crate::diag::Incident;
use crate::diag::scan::{diag_dirs, load_incidents};
use crate::eval::{Evaluation, Evaluator};
use crate::history::{self, HistorySample};
use crate::menu_model::fmt_gib;
use crate::metrics::{self, Snapshot};
use crate::processes::{self, Culprit, ProcInfo};

/// All inputs to one health report. Built by the I/O layer; read by pure functions.
#[derive(Debug, Clone)]
pub struct ReportInput {
    /// Unix seconds at which the report is generated.
    pub now: i64,
    pub boot_time: Option<i64>,
    /// e.g. `"Mac15,7 · macOS 27.2.1 · 18.0 GB RAM"`.
    pub machine: String,
    pub config: Config,
    pub snapshot: Snapshot,
    pub eval: Evaluation,
    /// Top 10 apps, grouped from a fresh process walk.
    pub apps: Vec<Culprit>,
    /// The full current process sample.
    pub procs: Vec<ProcInfo>,
    /// Incidents in the report window, oldest first.
    pub incidents: Vec<Incident>,
    /// History samples, oldest first.
    pub history: Vec<HistorySample>,
    /// Files read.
    pub sources: Vec<String>,
    /// Problems met while gathering (unreadable files and the like).
    pub notes: Vec<String>,
}

const LOG_DIR: &str = "Library/Logs/mac-health-monitor";
const REPORT_PREFIX: &str = "health-report-";
/// Window, in seconds, around the boot time in which a crash counts for the menu line.
const CRASH_WINDOW_SECS: i64 = 900;

fn log_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(LOG_DIR)
}

/// `$HOME/Library/Logs/mac-health-monitor/reports`.
pub fn reports_dir() -> PathBuf {
    log_dir().join("reports")
}

/// `$HOME/Library/Logs/mac-health-monitor/state.json`.
pub fn state_path() -> PathBuf {
    log_dir().join("state.json")
}

/// Reads a string-valued sysctl; `None` on any failure.
fn sysctl_string(name: &str) -> Option<String> {
    let cname = CString::new(name).ok()?;
    let mut len: usize = 0;
    // SAFETY: a null buffer asks the kernel only for the needed length, written to `len`.
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || len == 0 || len > 4096 {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: `buf` is a writable buffer of `len` bytes and `len` points to a valid usize.
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    buf.truncate(len.min(buf.len()));
    while buf.last() == Some(&0) {
        buf.pop();
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// e.g. `"Mac15,7 · macOS 27.2.1 · 18.0 GB RAM"`; `"unknown"` for what cannot be read.
pub fn machine_description(mem_total: u64) -> String {
    let model = sysctl_string("hw.model").unwrap_or_else(|| "unknown".into());
    let os = sysctl_string("kern.osproductversion")
        .map(|v| format!("macOS {v}"))
        .unwrap_or_else(|| "unknown".into());
    format!("{model} · {os} · {} RAM", fmt_gib(mem_total))
}

/// Gathers everything, renders the report, writes it and prunes old reports.
pub fn generate(config: &Config) -> Result<PathBuf, String> {
    let snapshot = metrics::sample()?;
    let eval = Evaluator::default().evaluate(&snapshot, config);
    let procs = processes::sample();
    let apps = processes::group(&procs, 10);
    let now = now_unix();
    let days = i64::try_from(config.report.incident_days).unwrap_or(30);
    let loaded = load_incidents(&diag_dirs(), now - days * 86_400);
    let input = ReportInput {
        now,
        boot_time: boot_time(),
        machine: machine_description(snapshot.mem_total_bytes),
        config: config.clone(),
        snapshot,
        eval,
        apps,
        procs,
        incidents: loaded.incidents,
        history: history::read(&history::history_path()),
        sources: loaded.sources,
        notes: loaded.notes,
    };
    let text = render::render(&input);
    let dir = reports_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let path = dir.join(format!(
        "{REPORT_PREFIX}{}.md",
        fmt_local(now, Pattern::FileStamp)
    ));
    fs::write(&path, text).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    prune(&dir, config.report.keep_reports);
    Ok(path)
}

/// Deletes all but the `keep` newest `health-report-*.md` files (newest by name).
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(REPORT_PREFIX) && n.ends_with(".md"))
        .collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    for name in names.into_iter().skip(keep) {
        if let Err(e) = fs::remove_file(dir.join(&name)) {
            eprintln!("could not remove old report {name}: {e}");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crash {
    pub time: i64,
    pub kind: CrashKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CrashKind {
    Panic,
    HangReset,
}

/// Whether the boot at `boot_time` followed a panic or a watchdog reset. A panic wins over a reset.
pub fn detect_crash(incidents: &[Incident], boot_time: i64) -> Option<Crash> {
    let panic = incidents.iter().find_map(|i| match i {
        Incident::Panic(p) => {
            let ok = if p.time_is_exact {
                (boot_time - CRASH_WINDOW_SECS..=boot_time).contains(&p.time)
            } else {
                (boot_time..=boot_time + CRASH_WINDOW_SECS).contains(&p.time)
            };
            ok.then_some(Crash {
                time: p.time,
                kind: CrashKind::Panic,
            })
        }
        _ => None,
    });
    panic.or_else(|| {
        incidents.iter().find_map(|i| match i {
            Incident::Reset(r)
                if r.is_watchdog()
                    && (boot_time..=boot_time + CRASH_WINDOW_SECS).contains(&r.time) =>
            {
                Some(Crash {
                    time: r.time,
                    kind: CrashKind::HangReset,
                })
            }
            _ => None,
        })
    })
}

/// The unacknowledged crash behind the current boot, if it is still within the report window.
pub fn startup_crash(config: &Config) -> Option<Crash> {
    let boot = boot_time()?;
    let loaded = load_incidents(&diag_dirs(), boot - 3600);
    let crash = detect_crash(&loaded.incidents, boot)?;
    let days = i64::try_from(config.report.incident_days).unwrap_or(30);
    if crash.time <= acknowledged() || crash.time < now_unix() - days * 86_400 {
        return None;
    }
    Some(crash)
}

#[derive(Deserialize)]
struct AckState {
    acknowledged_crash: i64,
}

fn read_ack(path: &Path) -> i64 {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<AckState>(&b).ok())
        .map_or(0, |s| s.acknowledged_crash)
}

fn write_ack(path: &Path, t: i64) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{{\"acknowledged_crash\": {t}}}\n"))
}

/// Crash time the user has already seen a report for; 0 if none.
pub fn acknowledged() -> i64 {
    read_ack(&state_path())
}

pub fn acknowledge(t: i64) -> std::io::Result<()> {
    write_ack(&state_path(), t)
}

/// The menu's first line after a crash-caused reboot.
pub fn crash_line(c: &Crash, now: i64) -> String {
    let when = fmt_relative(c.time, now);
    match c.kind {
        CrashKind::Panic => format!("⚠ Crashed at {when} — Health report…"),
        CrashKind::HangReset => format!("⚠ Restarted after a hang at {when} — Health report…"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::{Panic, Reset};

    const BOOT: i64 = 1_795_075_000;

    fn panic_at(time: i64, exact: bool) -> Incident {
        Incident::Panic(Panic {
            time,
            time_is_exact: exact,
            first_line: "x".into(),
            hung_service: None,
        })
    }

    fn reset_at(time: i64, faults: &str) -> Incident {
        Incident::Reset(Reset {
            time,
            boot_faults: faults.into(),
        })
    }

    #[test]
    fn detect_exact_panic_before_boot() {
        let c = detect_crash(&[panic_at(BOOT - 30, true)], BOOT);
        assert_eq!(
            c,
            Some(Crash {
                time: BOOT - 30,
                kind: CrashKind::Panic
            })
        );
        assert_eq!(detect_crash(&[panic_at(BOOT - 1000, true)], BOOT), None);
    }

    #[test]
    fn detect_inexact_panic_after_boot() {
        let c = detect_crash(&[panic_at(BOOT + 60, false)], BOOT);
        assert_eq!(c.map(|c| c.kind), Some(CrashKind::Panic));
        assert_eq!(detect_crash(&[panic_at(BOOT + 1000, false)], BOOT), None);
    }

    #[test]
    fn detect_reset_only() {
        let c = detect_crash(&[reset_at(BOOT + 62, "wdog,reset_in_1")], BOOT);
        assert_eq!(
            c,
            Some(Crash {
                time: BOOT + 62,
                kind: CrashKind::HangReset
            })
        );
        assert_eq!(
            detect_crash(&[reset_at(BOOT + 62, "reset_in_1")], BOOT),
            None
        );
    }

    #[test]
    fn detect_prefers_panic() {
        let c = detect_crash(
            &[reset_at(BOOT + 62, "wdog"), panic_at(BOOT - 30, true)],
            BOOT,
        );
        assert_eq!(c.map(|c| c.kind), Some(CrashKind::Panic));
    }

    #[test]
    fn ack_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mhm-ack-{}", std::process::id()));
        let path = dir.join("state.json");
        write_ack(&path, 123).unwrap();
        assert_eq!(read_ack(&path), 123);
        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(read_ack(&path), 0);
        assert_eq!(read_ack(&dir.join("missing.json")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_keeps_newest() {
        let dir = std::env::temp_dir().join(format!("mhm-prune-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for d in 11..=15 {
            std::fs::write(
                dir.join(format!("health-report-2026-11-{d}-000000.md")),
                "x",
            )
            .unwrap();
        }
        std::fs::write(dir.join("notes.md"), "x").unwrap();
        prune(&dir, 3);
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "health-report-2026-11-13-000000.md",
                "health-report-2026-11-14-000000.md",
                "health-report-2026-11-15-000000.md",
                "notes.md"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crash_line_wording() {
        let now = BOOT + 600;
        let p = Crash {
            time: BOOT - 30,
            kind: CrashKind::Panic,
        };
        let h = Crash {
            time: BOOT + 62,
            kind: CrashKind::HangReset,
        };
        assert!(crash_line(&p, now).starts_with("⚠ Crashed at "));
        assert!(crash_line(&p, now).ends_with(" — Health report…"));
        assert!(crash_line(&h, now).starts_with("⚠ Restarted after a hang at "));
    }

    #[test]
    #[ignore]
    fn smoke_generate() {
        let mut config = Config::default();
        config.report.incident_days = 30;
        let t = std::time::Instant::now();
        let path = generate(&config).unwrap();
        println!("report: {} in {:?}", path.display(), t.elapsed());
    }
}
