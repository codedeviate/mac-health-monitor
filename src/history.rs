//! Crash-surviving history of health samples, one JSON object per line.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::HistoryConfig;
use crate::eval::Evaluation;
use crate::menu_model::culprit_label;
use crate::metrics::Snapshot;
use crate::processes::Culprit;
use crate::status::{PressureLevel, Status};

const TRIM_EVERY_SECS: i64 = 3600;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistorySample {
    /// Unix time in seconds.
    pub t: i64,
    pub disk_free: u64,
    pub disk_total: u64,
    pub mem_avail: u64,
    pub mem_total: u64,
    /// Raw kernel pressure level: 1, 2 or 4.
    pub pressure: u8,
    pub swap_used: u64,
    pub swap_total: u64,
    pub compressed: u64,
    /// 0 Ok, 1 Warning, 2 Critical.
    pub disk_status: u8,
    pub memory_status: u8,
    pub swap_status: u8,
    /// Up to three top memory users: label (with "(×N)" when grouped) and bytes.
    pub top: Vec<(String, u64)>,
}

impl HistorySample {
    pub fn from_tick(t: i64, s: &Snapshot, e: &Evaluation, culprits: &[Culprit]) -> Self {
        HistorySample {
            t,
            disk_free: s.disk_free_bytes,
            disk_total: s.disk_total_bytes,
            mem_avail: s.mem_available_bytes,
            mem_total: s.mem_total_bytes,
            pressure: s.pressure.raw(),
            swap_used: s.swap_used_bytes,
            swap_total: s.swap_total_bytes,
            compressed: s.mem_compressed_bytes,
            disk_status: e.disk.as_u8(),
            memory_status: e.memory.as_u8(),
            swap_status: e.swap.as_u8(),
            top: culprits
                .iter()
                .take(3)
                .map(|c| (culprit_label(c), c.bytes))
                .collect(),
        }
    }

    pub fn disk_status(&self) -> Status {
        Status::from_u8(self.disk_status)
    }

    pub fn memory_status(&self) -> Status {
        Status::from_u8(self.memory_status)
    }

    pub fn swap_status(&self) -> Status {
        Status::from_u8(self.swap_status)
    }

    pub fn pressure_level(&self) -> PressureLevel {
        PressureLevel::from_raw(i32::from(self.pressure))
    }
}

/// Parses JSONL, skipping blank, unparsable or half-written lines; sorted by `t`.
pub fn parse_lines(text: &str) -> Vec<HistorySample> {
    let mut v: Vec<HistorySample> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l.trim()).ok())
        .collect();
    v.sort_by_key(|s| s.t);
    v
}

/// Splits into runs of samples; a gap over `3 * interval_secs` starts a new run.
pub fn segments(samples: &[HistorySample], interval_secs: u64) -> Vec<&[HistorySample]> {
    let max_gap = i64::try_from(interval_secs.saturating_mul(3)).unwrap_or(i64::MAX);
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..samples.len() {
        if samples[i].t.saturating_sub(samples[i - 1].t) > max_gap {
            out.push(&samples[start..i]);
            start = i;
        }
    }
    if start < samples.len() {
        out.push(&samples[start..]);
    }
    out
}

/// `$HOME/Library/Logs/mac-health-monitor/history.jsonl`.
pub fn history_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library")
        .join("Logs")
        .join("mac-health-monitor")
        .join("history.jsonl")
}

/// A missing or unreadable file gives no samples.
pub fn read(path: &Path) -> Vec<HistorySample> {
    match fs::read(path) {
        Ok(bytes) => parse_lines(&String::from_utf8_lossy(&bytes)),
        Err(_) => Vec::new(),
    }
}

/// Rewrites the file keeping only parsable samples newer than the retention window.
pub fn trim(path: &Path, now: i64, retain_hours: u64) -> std::io::Result<()> {
    let window = i64::try_from(retain_hours.saturating_mul(3600)).unwrap_or(i64::MAX);
    let cutoff = now.saturating_sub(window);
    let mut out = String::new();
    for s in read(path).iter().filter(|s| s.t >= cutoff) {
        if let Ok(line) = serde_json::to_string(s) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    let tmp = path.with_extension("jsonl.tmp");
    let mut f = File::create(&tmp)?;
    f.write_all(out.as_bytes())?;
    f.sync_data()?;
    fs::rename(&tmp, path)
}

/// Appends samples at the configured interval and trims old ones hourly.
pub struct Recorder {
    path: PathBuf,
    last_write: Option<i64>,
    last_trim: Option<i64>,
}

impl Recorder {
    pub fn new(path: PathBuf) -> Self {
        Recorder {
            path,
            last_write: None,
            last_trim: None,
        }
    }

    /// Returns whether a sample was written. `sample` is only built when due.
    pub fn observe(
        &mut self,
        now: i64,
        sample: impl FnOnce() -> HistorySample,
        cfg: &HistoryConfig,
    ) -> std::io::Result<bool> {
        let interval = i64::try_from(cfg.interval_secs).unwrap_or(i64::MAX);
        // A clock that jumped backwards (now < last) makes both due again.
        if let Some(last) = self.last_write
            && (0..interval).contains(&now.saturating_sub(last))
        {
            return Ok(false);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        if self
            .last_trim
            .is_none_or(|t| now < t || now.saturating_sub(t) >= TRIM_EVERY_SECS)
        {
            self.last_trim = Some(now);
            // Trimming is housekeeping; a failure must not block recording.
            let _ = trim(&self.path, now, cfg.retain_hours);
        }
        let mut line = serde_json::to_string(&sample()).map_err(std::io::Error::other)?;
        line.push('\n');
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(line.as_bytes())?;
        f.sync_data()?;
        self.last_write = Some(now);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t: i64) -> HistorySample {
        HistorySample {
            t,
            disk_free: 1,
            disk_total: 2,
            mem_avail: 3,
            mem_total: 4,
            pressure: 2,
            swap_used: 5,
            swap_total: 6,
            compressed: 7,
            disk_status: 0,
            memory_status: 1,
            swap_status: 2,
            top: vec![("claude (×3)".into(), 9)],
        }
    }

    fn line(t: i64) -> String {
        serde_json::to_string(&sample(t)).unwrap()
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mhm-history-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn line_roundtrip() {
        let s = sample(5);
        assert_eq!(parse_lines(&serde_json::to_string(&s).unwrap()), vec![s]);
    }

    #[test]
    fn parse_lines_skips_garbage_and_partial() {
        let good = line(1);
        let partial = &line(2)[..line(2).len() / 2];
        let text = format!("{good}\nnot json\n{partial}");
        assert_eq!(parse_lines(&text).len(), 1);
    }

    #[test]
    fn parse_lines_sorts_by_time() {
        let text = format!("{}\n{}\n{}\n", line(300), line(100), line(200));
        let ts: Vec<i64> = parse_lines(&text).iter().map(|s| s.t).collect();
        assert_eq!(ts, vec![100, 200, 300]);
    }

    #[test]
    fn segments_split_on_gap() {
        let v: Vec<_> = [0, 60, 120, 1000, 1060].into_iter().map(sample).collect();
        let segs = segments(&v, 60);
        assert_eq!(segs.iter().map(|s| s.len()).collect::<Vec<_>>(), vec![3, 2]);
    }

    #[test]
    fn read_missing_file_is_empty() {
        let d = tmp_dir("missing");
        assert!(read(&d.join("nope.jsonl")).is_empty());
    }

    #[test]
    fn recorder_respects_interval() {
        let d = tmp_dir("interval");
        let path = d.join("sub").join("history.jsonl");
        let cfg = HistoryConfig {
            interval_secs: 60,
            retain_hours: 24,
        };
        let mut r = Recorder::new(path.clone());
        let got: Vec<bool> = [1000, 1030, 1060]
            .into_iter()
            .map(|n| r.observe(n, || sample(n), &cfg).unwrap())
            .collect();
        assert_eq!(got, vec![true, false, true]);
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 2);
    }

    #[test]
    fn recorder_writes_after_clock_jumps_back() {
        let d = tmp_dir("clock-back");
        let path = d.join("history.jsonl");
        let cfg = HistoryConfig {
            interval_secs: 60,
            retain_hours: 24,
        };
        let mut r = Recorder::new(path.clone());
        assert!(r.observe(1000, || sample(1000), &cfg).unwrap());
        assert!(r.observe(500, || sample(500), &cfg).unwrap());
        assert_eq!(r.last_trim, Some(500));
        assert!(!r.observe(530, || sample(530), &cfg).unwrap());
    }

    #[test]
    fn trim_drops_old_lines() {
        let d = tmp_dir("trim");
        let path = d.join("history.jsonl");
        fs::write(&path, format!("{}\ngarbage\n{}\n", line(0), line(90_000))).unwrap();
        trim(&path, 90_000, 24).unwrap();
        let ts: Vec<i64> = read(&path).iter().map(|s| s.t).collect();
        assert_eq!(ts, vec![90_000]);
        assert!(!d.join("history.jsonl.tmp").exists());
    }
}
