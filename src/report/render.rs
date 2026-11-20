//! Renders a `ReportInput` as the Markdown health report (spec section 3).

use std::fmt::Write;

use crate::clock::{Pattern, fmt_duration, fmt_local};
use crate::diag::{Incident, Jetsam, Panic, Reset, Watchdog};
use crate::history::HistorySample;
use crate::menu_model::{Indicator, build, culprit_label, fmt_gb, fmt_gib};
use crate::status::Status;

use super::ReportInput;
use super::findings::{findings, fmt_mem, low_swap};

/// The timeline covers this long before its anchor.
const TIMELINE_SECS: i64 = 60 * 60;
/// One timeline row per this many seconds.
const TIMELINE_STEP_SECS: i64 = 5 * 60;
/// Apps listed per jetsam event.
const JETSAM_TOP_APPS: usize = 10;
const NO_DATA_ROW: &str = "| — | no data (asleep or app not running) | | | | |";

/// Renders the whole report. Never fails; missing data becomes explanatory text.
pub fn render(input: &ReportInput) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Health report — {}\n",
        fmt_local(input.now, Pattern::DateHm)
    );
    header(&mut out, input);

    out.push_str("\n## What this suggests\n\n");
    for f in findings(input) {
        let _ = writeln!(out, "- {f}");
    }

    incidents(&mut out, input);
    timeline(&mut out, input);
    now_section(&mut out, input);
    sources(&mut out, input);
    out
}

/// Makes text safe inside a Markdown table cell.
fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn header(out: &mut String, input: &ReportInput) {
    let mut line = format!("- Generated: {}", fmt_local(input.now, Pattern::DateTime));
    if let Some(boot) = input.boot_time {
        let _ = write!(
            line,
            " · Uptime: {} (booted {})",
            fmt_duration(input.now - boot),
            fmt_local(boot, Pattern::DateTime)
        );
    }
    let _ = writeln!(out, "{line}\n- Machine: {}", input.machine);
}

fn incidents(out: &mut String, input: &ReportInput) {
    let _ = writeln!(
        out,
        "\n## Incidents (last {} days)\n",
        input.config.report.incident_days
    );
    if input.incidents.is_empty() {
        out.push_str("None in this period.\n");
        return;
    }
    let mut sorted: Vec<&Incident> = input.incidents.iter().collect();
    sorted.sort_by_key(|i| std::cmp::Reverse(i.time()));
    for inc in sorted {
        match inc {
            Incident::Panic(p) => panic_section(out, p),
            Incident::Jetsam(j) => jetsam_section(out, j),
            Incident::Watchdog(w) => watchdog_section(out, w),
            Incident::Reset(r) => reset_section(out, r),
        }
    }
}

fn panic_section(out: &mut String, p: &Panic) {
    let reported = if p.time_is_exact {
        ""
    } else {
        " (reported at)"
    };
    let _ = writeln!(
        out,
        "### {}{reported} — Kernel panic\n\n> {}\n",
        fmt_local(p.time, Pattern::DateHm),
        p.first_line.replace('\n', " ")
    );
}

fn jetsam_section(out: &mut String, j: &Jetsam) {
    let _ = writeln!(
        out,
        "### {} — Memory pressure (jetsam)\n",
        fmt_local(j.time, Pattern::DateHm)
    );
    let mut line = format!(
        "Free {} · Compressor {}",
        fmt_mem(j.free_bytes),
        fmt_mem(j.compressor_bytes)
    );
    if !j.kills.is_empty() {
        let kills: Vec<String> = j.kills.iter().map(|(r, n)| format!("{n} {r}")).collect();
        let _ = write!(line, " · Killed: {}", kills.join(", "));
    }
    let _ = writeln!(out, "{line}");
    let mut limited: Vec<&str> = j.per_process_limit.iter().map(String::as_str).collect();
    limited.sort_unstable();
    limited.dedup();
    if !limited.is_empty() {
        let _ = writeln!(out, "\nHit their own memory limit: {}", limited.join(", "));
    }
    if !j.groups.is_empty() {
        out.push_str("\n| App | Resident |\n|---|---|\n");
        for g in j.groups.iter().take(JETSAM_TOP_APPS) {
            let label = if g.count > 1 {
                format!("{} (×{})", g.label, g.count)
            } else {
                g.label.clone()
            };
            let _ = writeln!(out, "| {} | {} |", cell(&label), fmt_gib(g.bytes));
        }
    }
    out.push('\n');
}

fn watchdog_section(out: &mut String, w: &Watchdog) {
    let _ = writeln!(
        out,
        "### {} — Watchdog timeout\n\nService: {} · {}\n",
        fmt_local(w.time, Pattern::DateHm),
        w.service.as_deref().unwrap_or("unknown"),
        w.reason.replace('\n', " ")
    );
}

fn reset_section(out: &mut String, r: &Reset) {
    let _ = writeln!(
        out,
        "### {} — Reset\n\nBoot faults: {}\n",
        fmt_local(r.time, Pattern::DateHm),
        r.boot_faults.replace('\n', " ")
    );
}

/// What the timeline looks back from: the newest crash (panic or hang-reset), else the
/// newest memory-pressure event with low-swap kills, else now.
fn timeline_anchor(input: &ReportInput) -> Option<(i64, &'static str)> {
    let crash = input
        .incidents
        .iter()
        .filter_map(|i| match i {
            Incident::Panic(p) => Some((p.time, "the kernel panic")),
            Incident::Reset(r) if r.is_watchdog() => Some((r.time, "the hang-reset")),
            _ => None,
        })
        .max_by_key(|(t, _)| *t);
    crash.or_else(|| {
        input
            .incidents
            .iter()
            .filter_map(|i| match i {
                Incident::Jetsam(j) if low_swap(j) > 0 => {
                    Some((j.time, "the memory-pressure kills"))
                }
                _ => None,
            })
            .max_by_key(|(t, _)| *t)
    })
}

fn timeline(out: &mut String, input: &ReportInput) {
    if let Some((anchor, what)) = timeline_anchor(input) {
        let _ = writeln!(
            out,
            "\n## Before {what} at {}\n",
            fmt_local(anchor, Pattern::DateHm)
        );
        if timeline_table(out, input, anchor) {
            return;
        }
    }
    out.push_str("\n## Last 60 minutes\n\n");
    timeline_table(out, input, input.now);
}

/// The 60 minutes of history before `end`, or a line saying there is none.
/// Returns whether there was any history.
fn timeline_table(out: &mut String, input: &ReportInput, end: i64) -> bool {
    let start = end - TIMELINE_SECS;
    let samples: Vec<&HistorySample> = input
        .history
        .iter()
        .filter(|s| s.t >= start && s.t <= end)
        .collect();
    if samples.is_empty() {
        out.push_str("No history available for this period.\n");
        return false;
    }

    let max_gap =
        i64::try_from(input.config.history.interval_secs.saturating_mul(3)).unwrap_or(i64::MAX);
    out.push_str(
        "| Time | Disk free | Mem avail | Pressure | Swap used | Top culprit |\n|---|---|---|---|---|---|\n",
    );
    let mut in_gap = false;
    let mut mark = start;
    while mark <= end {
        let nearest = samples.iter().min_by_key(|s| (s.t - mark).abs());
        match nearest.filter(|s| (s.t - mark).abs() <= max_gap) {
            Some(s) => {
                in_gap = false;
                let _ = writeln!(
                    out,
                    "| {} | {} |",
                    fmt_local(s.t, Pattern::Hm),
                    sample_cells(s)
                );
            }
            None if !in_gap => {
                in_gap = true;
                let _ = writeln!(out, "{NO_DATA_ROW}");
            }
            None => {}
        }
        mark += TIMELINE_STEP_SECS;
    }
    worst_row(out, &samples);
    true
}

/// The five value cells of a timeline row.
fn sample_cells(s: &HistorySample) -> String {
    let pct = if s.mem_total == 0 {
        0.0
    } else {
        s.mem_avail as f64 / s.mem_total as f64 * 100.0
    };
    let top = s
        .top
        .first()
        .map(|(label, bytes)| format!("{} {}", cell(label), fmt_gib(*bytes)))
        .unwrap_or_default();
    format!(
        "{} | {} ({pct:.0}%) | {} | {} | {top}",
        fmt_gb(s.disk_free),
        fmt_gib(s.mem_avail),
        s.pressure_level().label(),
        fmt_gib(s.swap_used),
    )
}

/// Minimum disk free, minimum memory available and maximum swap in the period.
fn worst_row(out: &mut String, samples: &[&HistorySample]) {
    let disk = samples.iter().map(|s| s.disk_free).min().unwrap_or(0);
    let mem = samples.iter().map(|s| s.mem_avail).min().unwrap_or(0);
    let swap = samples.iter().map(|s| s.swap_used).max().unwrap_or(0);
    let _ = writeln!(
        out,
        "| **Worst** | {} | {} | | {} | |",
        fmt_gb(disk),
        fmt_gib(mem),
        fmt_gib(swap)
    );
}

fn status_text(i: Indicator) -> &'static str {
    match i {
        Indicator::Status(Status::Ok) => "OK",
        Indicator::Status(Status::Warning) => "Warning",
        Indicator::Status(Status::Critical) => "Critical",
        Indicator::Info | Indicator::None => "",
    }
}

fn now_section(out: &mut String, input: &ReportInput) {
    let model = build(&input.snapshot, &input.eval, &input.apps, None);
    out.push_str("\n## Now\n\n| Parameter | Value | Status |\n|---|---|---|\n");
    for r in &model.params {
        let _ = writeln!(
            out,
            "| {} | {} | {} |",
            cell(&r.label),
            cell(&r.value),
            status_text(r.indicator)
        );
    }
    if !input.apps.is_empty() {
        out.push_str("\n| App | Resident |\n|---|---|\n");
        for c in &input.apps {
            let _ = writeln!(
                out,
                "| {} | {} |",
                cell(&culprit_label(c)),
                fmt_gib(c.bytes)
            );
        }
    }
}

fn sources(out: &mut String, input: &ReportInput) {
    out.push_str("\n## Sources\n\n");
    if input.sources.is_empty() && input.notes.is_empty() {
        out.push_str("No files were read.\n");
    }
    for s in &input.sources {
        let _ = writeln!(out, "- {s}");
    }
    for n in &input.notes {
        let _ = writeln!(out, "- Note: {n}");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::Config;
    use crate::diag::{Incident, Jetsam, Panic, ProcGroup, Reset, Watchdog};
    use crate::eval::Evaluation;
    use crate::history::HistorySample;
    use crate::metrics::Snapshot;
    use crate::processes::Culprit;
    use crate::status::{PressureLevel, Status};

    const NOW: i64 = 1_795_080_000;
    const GIB_U: u64 = 1 << 30;

    fn input() -> ReportInput {
        ReportInput {
            now: NOW,
            boot_time: Some(NOW - 600),
            machine: "Mac15,7 · macOS 27.2.1 · 18.0 GB RAM".into(),
            config: Config::default(),
            snapshot: Snapshot {
                disk_free_bytes: 200_000_000_000,
                disk_total_bytes: 500_000_000_000,
                mem_total_bytes: 18 * GIB_U,
                mem_available_bytes: 9 * GIB_U,
                mem_compressed_bytes: GIB_U,
                pressure: PressureLevel::Normal,
                swap_used_bytes: 0,
                swap_total_bytes: 0,
            },
            eval: Evaluation {
                disk: Status::Ok,
                memory: Status::Ok,
                swap: Status::Ok,
                pressure: Status::Ok,
            },
            apps: vec![],
            procs: vec![],
            incidents: vec![],
            history: vec![],
            sources: vec![],
            notes: vec![],
        }
    }

    fn sample(t: i64) -> HistorySample {
        HistorySample {
            t,
            disk_free: 200_000_000_000,
            disk_total: 500_000_000_000,
            mem_avail: 9 * GIB_U,
            mem_total: 18 * GIB_U,
            pressure: 1,
            swap_used: 0,
            swap_total: 0,
            compressed: GIB_U,
            disk_status: 0,
            memory_status: 0,
            swap_status: 0,
            top: vec![("Zen".into(), 3 * GIB_U)],
        }
    }

    fn panic_at(time: i64) -> Incident {
        Incident::Panic(Panic {
            time,
            time_is_exact: true,
            first_line: "userspace watchdog timeout: no successful checkins from WindowServer"
                .into(),
            hung_service: Some("WindowServer".into()),
        })
    }

    fn jetsam_at(time: i64) -> Incident {
        Incident::Jetsam(Jetsam {
            time,
            free_bytes: 170 << 20,
            compressor_bytes: 7 * GIB_U,
            largest_process: None,
            kills: BTreeMap::from([("low-swap".to_string(), 3)]),
            per_process_limit: vec![],
            groups: vec![ProcGroup {
                name: "plugin-container".into(),
                label: "Firefox/Zen content processes".into(),
                bytes: 10 * GIB_U,
                count: 2,
            }],
        })
    }

    #[test]
    fn render_empty_inputs() {
        let out = render(&input());
        assert!(out.contains("# Health report — "), "{out}");
        assert!(
            out.contains("No incidents in the last 7 days and nothing unusual right now."),
            "{out}"
        );
        assert!(
            out.contains("No history available for this period."),
            "{out}"
        );
        assert!(out.contains("## Now"), "{out}");
        assert!(out.contains("## Sources"), "{out}");
    }

    #[test]
    fn render_panic_and_jetsam() {
        let mut i = input();
        i.incidents = vec![jetsam_at(NOW - 900), panic_at(NOW - 300)];
        let out = render(&i);
        let p = out.find("Kernel panic").expect("panic heading");
        let j = out
            .find("Memory pressure (jetsam)")
            .expect("jetsam heading");
        assert!(p < j, "{out}");
        assert!(out.contains("Firefox/Zen content processes (×2)"), "{out}");
    }

    #[test]
    fn render_timeline_with_gap() {
        let mut i = input();
        i.incidents = vec![panic_at(NOW)];
        i.history = (0..=20)
            .map(|m| NOW - (60 - m) * 60)
            .chain((0..=20).map(|m| NOW - (20 - m) * 60))
            .map(sample)
            .collect();
        let out = render(&i);
        assert_eq!(out.matches("no data").count(), 1, "{out}");
        assert!(out.contains("Worst"), "{out}");
    }

    fn idle_exit_jetsam_at(time: i64) -> Incident {
        let Incident::Jetsam(mut j) = jetsam_at(time) else {
            unreachable!()
        };
        j.kills = BTreeMap::from([("idle-exit".to_string(), 30)]);
        Incident::Jetsam(j)
    }

    fn spin_at(time: i64) -> Incident {
        Incident::Watchdog(Watchdog {
            time,
            service: Some("configd".into()),
            reason: "slow".into(),
        })
    }

    #[test]
    fn timeline_anchored_on_panic_not_later_noise() {
        let mut i = input();
        let p = NOW - 3 * 3600;
        i.incidents = vec![
            panic_at(p),
            idle_exit_jetsam_at(NOW - 120),
            spin_at(NOW - 60),
        ];
        i.history = (0..=60).map(|m| p - m * 60).map(sample).collect();
        let out = render(&i);
        let heading = format!(
            "## Before the kernel panic at {}",
            fmt_local(p, Pattern::DateHm)
        );
        assert!(out.contains(&heading), "{out}");
        assert!(!out.contains("Before the last incident"), "{out}");
        assert!(out.contains("Worst"), "{out}");
        assert!(!out.contains("## Last 60 minutes"), "{out}");
    }

    #[test]
    fn timeline_heading_names_reset_and_kills() {
        let mut i = input();
        i.incidents = vec![
            jetsam_at(NOW - 600),
            Incident::Reset(Reset {
                time: NOW - 1200,
                boot_faults: "wdog,reset_in_1".into(),
            }),
        ];
        let out = render(&i);
        let heading = format!(
            "## Before the hang-reset at {}",
            fmt_local(NOW - 1200, Pattern::DateHm)
        );
        assert!(out.contains(&heading), "{out}");

        i.incidents = vec![jetsam_at(NOW - 600), idle_exit_jetsam_at(NOW - 60)];
        let out = render(&i);
        let heading = format!(
            "## Before the memory-pressure kills at {}",
            fmt_local(NOW - 600, Pattern::DateHm)
        );
        assert!(out.contains(&heading), "{out}");

        i.incidents = vec![idle_exit_jetsam_at(NOW - 60), spin_at(NOW - 30)];
        let out = render(&i);
        assert!(out.contains("## Last 60 minutes"), "{out}");
        assert!(!out.contains("## Before"), "{out}");
    }

    #[test]
    fn timeline_without_anchor_history_also_shows_last_hour() {
        let mut i = input();
        let p = NOW - 2 * 86_400;
        i.incidents = vec![panic_at(p)];
        i.history = (0..=60).map(|m| NOW - m * 60).map(sample).collect();
        let out = render(&i);
        let before = out
            .find("## Before the kernel panic at")
            .expect("anchor heading");
        let none = out
            .find("No history available for this period.")
            .expect("no-history line");
        let last = out.find("## Last 60 minutes").expect("last-hour heading");
        assert!(before < none && none < last, "{out}");
        assert!(out[last..].contains("Worst"), "{out}");
    }

    #[test]
    fn render_escapes_pipes_in_cells() {
        let mut i = input();
        i.apps = vec![Culprit {
            label: "a|b".into(),
            bytes: GIB_U,
            count: 1,
        }];
        let out = render(&i);
        assert!(out.contains("a\\|b"), "{out}");
    }
}
