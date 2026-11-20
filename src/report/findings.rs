//! "What this suggests": plain-language findings derived from incidents,
//! the current state and the recent history (spec section 4, rules 1–10).
//!
//! Findings are hedged and cite the numbers that triggered them. Each rule is
//! a small function; `findings` calls them in spec order.

use std::collections::BTreeMap;

use crate::clock::{Pattern, fmt_duration, fmt_local, fmt_relative};
use crate::diag::{Incident, Jetsam, Panic, Watchdog, display_name};
use crate::history::{HistorySample, segments};
use crate::menu_model::{culprit_label, fmt_gb, fmt_gib};
use crate::processes::ProcInfo;
use crate::status::Status;
use crate::units::{GB, GIB};

use super::ReportInput;

/// Rule 3: a jetsam this long before a panic counts as "memory ran out first".
const JETSAM_BEFORE_PANIC_SECS: i64 = 15 * 60;
/// Rule 2: a panic this soon after a watchdog timeout means the two are one event.
const WATCHDOG_PANIC_LINK_SECS: i64 = 10 * 60;
/// Rule 4: history is checked this far either side of a low-swap jetsam.
const DISK_CHECK_SECS: i64 = 10 * 60;
/// Rule 6: an app above this share of RAM is called out.
const BIG_APP_SHARE_PCT: f64 = 30.0;
/// Rule 7: a system service above this resident size is unusual.
const SERVICE_MIN_BYTES: u64 = 1 << 30;
/// Rule 8: trends are judged over this window before now.
const TREND_WINDOW_SECS: i64 = 60 * 60;
/// Rules on history need at least this much uninterrupted coverage.
const MIN_COVERAGE_SECS: i64 = 30 * 60;
/// Rule 8: swap growth worth mentioning, bytes per hour (2 GB, binary).
const SWAP_GROWTH_PER_H: f64 = 2.0 * GIB;
/// Rule 8: disk shrink worth mentioning, bytes per hour (5 GB, decimal).
const DISK_FALL_PER_H: f64 = 5.0 * GB;
/// Rule 8: projections further out than this are not worth stating.
const MAX_PROJECTION_H: f64 = 48.0;
/// Rule 9: swap red this long without a break suggests a restart.
const SWAP_RED_SECS: i64 = 30 * 60;
/// Rule 9: uptime beyond this, with swap yellow or red, suggests a restart.
const LONG_UPTIME_SECS: i64 = 7 * 86_400;
/// Paths of current processes that count as system services (rule 7).
const SYSTEM_DIRS: [&str; 3] = ["/System/", "/usr/libexec/", "/usr/sbin/"];
/// Jetsam names known to be Apple system daemons (rule 7).
const KNOWN_SERVICES: [&str; 22] = [
    "sysinfod",
    "mds",
    "mds_stores",
    "mdworker",
    "mdworker_shared",
    "mediaanalysisd",
    "photoanalysisd",
    "photolibraryd",
    "cloudd",
    "bird",
    "softwareupdated",
    "backupd",
    "corespotlightd",
    "spotlightknowledged",
    "logd",
    "configd",
    "coreaudiod",
    "cfprefsd",
    "trustd",
    "nsurlsessiond",
    "fileproviderd",
    "syspolicyd",
];
const LOW_SWAP: &str = "low-swap";

/// Findings in rule order; never empty.
pub fn findings(input: &ReportInput) -> Vec<String> {
    let mut out = Vec::new();
    out.extend(rule_panics(input));
    out.extend(rule_watchdogs(input));
    out.extend(rule_memory_before_panic(input));
    out.extend(rule_swap_exhausted(input));
    out.extend(rule_per_process_limit(input));
    out.extend(rule_biggest_users(input));
    out.extend(rule_system_services(input));
    out.extend(rule_trends(input));
    out.extend(rule_time_to_restart(input));
    if out.is_empty() {
        out.push(rule_nothing_found(input));
    }
    out
}

/// Current processes above `min_bytes` that run from a system location,
/// largest first. Apps, WindowServer, `kernel_task`, the virtualization
/// process and WebKit content processes are left out: their size is normal.
pub fn system_services(procs: &[ProcInfo], min_bytes: u64) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = procs
        .iter()
        .filter(|p| p.footprint_bytes > min_bytes)
        .filter(|p| SYSTEM_DIRS.iter().any(|d| p.path.starts_with(d)))
        .filter(|p| {
            !p.path.contains(".app/")
                && !p.path.contains("com.apple.Virtualization")
                && !p.path.contains("com.apple.WebKit")
        })
        .filter_map(|p| {
            let name = p.path.rsplit('/').next().unwrap_or("");
            (!name.is_empty() && !is_excluded_service(name))
                .then(|| (name.to_string(), p.footprint_bytes))
        })
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Whether a bare jetsam process name is a known Apple system service:
/// a built-in list of daemons plus `com.apple.*` names.
pub fn is_system_service_name(name: &str) -> bool {
    !is_excluded_service(name) && (KNOWN_SERVICES.contains(&name) || name.starts_with("com.apple."))
}

/// Apple processes whose large size is normal.
fn is_excluded_service(name: &str) -> bool {
    name == "WindowServer"
        || name == "kernel_task"
        || name.starts_with("com.apple.Virtualization")
        || name.starts_with("com.apple.WebKit")
}

/// Hours until `value_now`, changing at `rate_per_hour`, reaches `limit`.
/// `None` when it is moving away from the limit, not moving, or already past it.
pub fn hours_until(value_now: f64, rate_per_hour: f64, limit: f64) -> Option<f64> {
    if rate_per_hour == 0.0 {
        return None;
    }
    let h = (limit - value_now) / rate_per_hour;
    (h.is_finite() && h > 0.0).then_some(h)
}

/// Memory size: whole MB (binary) below 1 GiB, otherwise GB with one decimal.
pub fn fmt_mem(bytes: u64) -> String {
    if (bytes as f64) < GIB {
        format!("{:.0} MB", bytes as f64 / (1u64 << 20) as f64)
    } else {
        fmt_gib(bytes)
    }
}

// ---- rules, in spec order ----

/// Rule 1: each kernel panic, translated when it is a userspace watchdog timeout.
fn rule_panics(input: &ReportInput) -> Vec<String> {
    panics(input)
        .map(|p| {
            let at = if p.time_is_exact {
                format!("at {}", when(p.time, input.now))
            } else {
                format!(
                    "(panic reported at {}; the exact time is unknown)",
                    when(p.time, input.now)
                )
            };
            match &p.hung_service {
                Some(svc) if p.first_line.contains("userspace watchdog timeout") => {
                    let dur = seconds_in(&p.first_line)
                        .map(|s| format!(" for {s} s"))
                        .unwrap_or_default();
                    format!(
                        "The Mac froze and restarted itself {at}: {svc} stopped responding{dur} (kernel panic, watchdog timeout)."
                    )
                }
                _ => format!(
                    "The Mac had a kernel panic {at}: \"{}\".",
                    truncate(&p.first_line, 200)
                ),
            }
        })
        .collect()
}

/// Rule 2: a watchdog timeout that no panic followed.
fn rule_watchdogs(input: &ReportInput) -> Vec<String> {
    watchdogs(input)
        .filter(|w| {
            !panics(input).any(|p| (0..=WATCHDOG_PANIC_LINK_SECS).contains(&(p.time - w.time)))
        })
        .map(|w| {
            let who = w
                .service
                .as_deref()
                .map_or("A system service".to_string(), |s| format!("Service {s}"));
            let dur = seconds_in(&w.reason)
                .map(|s| format!(" for {s} s"))
                .unwrap_or_default();
            format!(
                "{who} was unresponsive{dur} at {} (watchdog timeout; no kernel panic followed).",
                when(w.time, input.now)
            )
        })
        .collect()
}

/// Rule 3: a low-swap jetsam within 15 minutes before a panic.
fn rule_memory_before_panic(input: &ReportInput) -> Vec<String> {
    panics(input)
        .filter_map(|p| {
            let j = jetsams(input)
                .filter(|j| low_swap(j) > 0 && precedes(j, p))
                .max_by_key(|j| j.time)?;
            Some(format!(
                "Before the panic at {}, memory ran out first: at {} macOS killed {} because swap was exhausted (\"low-swap\"), with about {} of RAM free.",
                when(p.time, input.now),
                when(j.time, input.now),
                count(low_swap(j), "process", "processes"),
                fmt_mem(j.free_bytes)
            ))
        })
        .collect()
}

/// Rule 4: swap could not grow (low-swap kills), and whether a nearly full
/// disk is the likely reason.
fn rule_swap_exhausted(input: &ReportInput) -> Option<String> {
    let events: Vec<&Jetsam> = jetsams(input).filter(|j| low_swap(j) > 0).collect();
    let latest = events.iter().max_by_key(|j| j.time)?;
    let also = if events.len() > 1 {
        format!(
            " (one of {} such events in the last {})",
            events.len(),
            days(input)
        )
    } else {
        String::new()
    };
    let mut newest_first = events.clone();
    newest_first.sort_by_key(|j| std::cmp::Reverse(j.time));
    if let Some((j, s)) = newest_first
        .iter()
        .find_map(|j| disk_nearly_full(input, j).map(|s| (j, s)))
    {
        // Rule 3 already cites the kills of a jetsam right before a panic.
        let killed = if panics(input).any(|p| precedes(j, p)) {
            String::new()
        } else {
            format!(
                " (macOS killed {} for \"low-swap\")",
                count(low_swap(j), "process", "processes")
            )
        };
        return Some(format!(
            "At {} swap could not grow{}, likely because the disk was nearly full ({} free at {}){also}.",
            when(j.time, input.now),
            killed,
            fmt_gb(s.disk_free),
            fmt_local(s.t, Pattern::Hm)
        ));
    }
    // Rule 3 already described a jetsam right before a panic.
    if panics(input).any(|p| precedes(latest, p)) {
        return None;
    }
    Some(format!(
        "Swap ran out at {}: macOS killed {} because swap could not grow (\"low-swap\"), with about {} of RAM free{also}.",
        when(latest.time, input.now),
        count(low_swap(latest), "process", "processes"),
        fmt_mem(latest.free_bytes)
    ))
}

/// Rule 5: each process killed for exceeding its own memory limit.
fn rule_per_process_limit(input: &ReportInput) -> Vec<String> {
    // name -> (kills, latest time), in order of first appearance.
    let mut order: Vec<&str> = Vec::new();
    let mut seen: BTreeMap<&str, (usize, i64)> = BTreeMap::new();
    for j in jetsams(input) {
        for name in &j.per_process_limit {
            let e = seen.entry(name).or_insert_with(|| {
                order.push(name);
                (0, j.time)
            });
            e.0 += 1;
            e.1 = e.1.max(j.time);
        }
    }
    order
        .into_iter()
        .filter_map(|name| {
            let (n, last) = seen.get(name)?;
            let at = when(*last, input.now);
            let tail = if *n > 1 {
                format!("{n} times (last at {at})")
            } else {
                format!("at {at}")
            };
            Some(format!(
                "{} hit its own memory limit and was killed {tail}.",
                display_name(name)
            ))
        })
        .collect()
}

/// Rule 6: top 3 memory users from the most recent jetsam, or from now when
/// there is none and memory, swap or pressure is not OK.
fn rule_biggest_users(input: &ReportInput) -> Option<String> {
    let (at, verb, items): (String, &str, Vec<(String, u64)>) =
        if let Some(j) = latest_jetsam(input) {
            let items = j
                .groups
                .iter()
                .take(3)
                .map(|g| (counted(&g.label, g.count), g.bytes))
                .collect();
            (format!("at {}", when(j.time, input.now)), "was", items)
        } else if struggling(input) {
            let items = input
                .apps
                .iter()
                .take(3)
                .map(|c| (culprit_label(c), c.bytes))
                .collect();
            ("now".to_string(), "is", items)
        } else {
            return None;
        };
    if items.is_empty() {
        return None;
    }
    let total = input.snapshot.mem_total_bytes;
    let share = |b: u64| (total > 0).then(|| 100.0 * b as f64 / total as f64);
    let list: Vec<String> = items
        .iter()
        .map(|(label, b)| match share(*b) {
            Some(pct) => format!("{label} {} ({pct:.0}% of RAM)", fmt_gib(*b)),
            None => format!("{label} {}", fmt_gib(*b)),
        })
        .collect();
    let mut s = format!("Biggest memory users {at}: {}", list.join(", "));
    let big: Vec<&str> = items
        .iter()
        .filter(|(_, b)| share(*b).is_some_and(|p| p > BIG_APP_SHARE_PCT))
        .map(|(l, _)| l.as_str())
        .collect();
    match big.as_slice() {
        [] => {}
        [one] => s.push_str(&format!(
            "; {one} alone {verb} using more than 30% of the {} of RAM",
            fmt_gib(total)
        )),
        many => s.push_str(&format!(
            "; {} {} each using more than 30% of the {} of RAM",
            many.join(" and "),
            if verb == "was" { "were" } else { "are" },
            fmt_gib(total)
        )),
    }
    s.push('.');
    Some(s)
}

/// Rule 7: Apple system services above 1 GiB, in the most recent jetsam or now.
fn rule_system_services(input: &ReportInput) -> Option<String> {
    // (name, bytes, when, verb)
    let mut items: Vec<(String, u64, String, &str)> = Vec::new();
    if let Some(j) = latest_jetsam(input) {
        for g in &j.groups {
            if g.bytes > SERVICE_MIN_BYTES && is_system_service_name(&g.name) {
                items.push((
                    counted(&g.name, g.count),
                    g.bytes,
                    format!("at {}", when(j.time, input.now)),
                    "was",
                ));
            }
        }
    }
    for (name, bytes) in system_services(&input.procs, SERVICE_MIN_BYTES) {
        items.push((name, bytes, "now".to_string(), "is"));
    }
    match items.as_slice() {
        [] => None,
        [(name, bytes, at, verb)] => Some(format!(
            "{name} (a system service) {verb} using {} {at}, which is unusual for it.",
            fmt_gib(*bytes)
        )),
        many => Some(format!(
            "System services using an unusual amount of memory (over 1 GB): {}.",
            many.iter()
                .map(|(name, bytes, at, _)| format!("{name} {} {at}", fmt_gib(*bytes)))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Rule 8: swap growth and disk shrink over the last hour of history.
fn rule_trends(input: &ReportInput) -> Vec<String> {
    let mut out = Vec::new();
    let window = in_window(input, TREND_WINDOW_SECS);
    let Some(run) = current_run(input, window) else {
        return out;
    };
    let (first, last) = (&run[0], &run[run.len() - 1]);
    let span = last.t - first.t;
    let hours = span as f64 / 3600.0;
    let red = input.config.disk.crit_free_gb * GB;
    let disk_now = input.snapshot.disk_free_bytes as f64;

    let swap_rate = (last.swap_used as f64 - first.swap_used as f64) / hours;
    if swap_rate > SWAP_GROWTH_PER_H {
        out.push(format!(
            "Swap has grown {} in the last {} ({:.1} GB/h){}.",
            fmt_gib(last.swap_used.saturating_sub(first.swap_used)),
            fmt_duration(span),
            swap_rate / GIB,
            disk_projection(disk_now, swap_rate, red, "the disk")
        ));
    }

    let disk_rate = (first.disk_free as f64 - last.disk_free as f64) / hours;
    if disk_rate > DISK_FALL_PER_H {
        out.push(format!(
            "Disk free space has fallen {} in the last {} ({:.1} GB/h){}.",
            fmt_gb(first.disk_free.saturating_sub(last.disk_free)),
            fmt_duration(span),
            disk_rate / GB,
            disk_projection(disk_now, disk_rate, red, "it")
        ));
    }
    out
}

/// Rule 9: swap red for 30 minutes without a break, or a long uptime with
/// swap yellow or red.
fn rule_time_to_restart(input: &ReportInput) -> Option<String> {
    let uptime = input.boot_time.map(|b| input.now - b);
    let red_for = current_run(input, &input.history).and_then(|run| {
        let start = run
            .iter()
            .rposition(|s| s.swap_status() != Status::Critical)
            .map_or(0, |i| i + 1);
        let red = &run[start..];
        let last = red.last()?;
        let secs = last.t - red.first()?.t;
        (secs >= SWAP_RED_SECS).then_some((secs, last.swap_used))
    });
    if let Some((secs, used)) = red_for {
        let up = uptime
            .map(|u| format!(" (uptime {})", fmt_duration(u)))
            .unwrap_or_default();
        return Some(format!(
            "Swap has been red for {} without a break ({} used); a restart would likely help{up}.",
            fmt_duration(secs),
            fmt_gib(used)
        ));
    }
    let up = uptime.filter(|u| *u > LONG_UPTIME_SECS)?;
    let colour = match input.eval.swap {
        Status::Ok => return None,
        Status::Warning => "yellow",
        Status::Critical => "red",
    };
    Some(format!(
        "The Mac has been up for {} and swap is {colour} ({} used); a restart would likely help.",
        fmt_duration(up),
        fmt_gib(input.snapshot.swap_used_bytes)
    ))
}

/// Rule 10: said only when no other rule produced anything.
fn rule_nothing_found(input: &ReportInput) -> String {
    let e = &input.eval;
    let all_ok = [e.disk, e.memory, e.swap, e.pressure]
        .iter()
        .all(|s| *s == Status::Ok);
    match (input.incidents.is_empty(), all_ok) {
        (true, true) => format!(
            "No incidents in the last {} and nothing unusual right now.",
            days(input)
        ),
        (true, false) => format!("No incidents in the last {}.", days(input)),
        (false, _) => "No clear cause stands out in the incidents listed below.".to_string(),
    }
}

// ---- helpers ----

fn panics(input: &ReportInput) -> impl Iterator<Item = &Panic> {
    input.incidents.iter().filter_map(|i| match i {
        Incident::Panic(p) => Some(p),
        _ => None,
    })
}

fn watchdogs(input: &ReportInput) -> impl Iterator<Item = &Watchdog> {
    input.incidents.iter().filter_map(|i| match i {
        Incident::Watchdog(w) => Some(w),
        _ => None,
    })
}

fn jetsams(input: &ReportInput) -> impl Iterator<Item = &Jetsam> {
    input.incidents.iter().filter_map(|i| match i {
        Incident::Jetsam(j) => Some(j),
        _ => None,
    })
}

fn latest_jetsam(input: &ReportInput) -> Option<&Jetsam> {
    jetsams(input).max_by_key(|j| j.time)
}

fn low_swap(j: &Jetsam) -> usize {
    j.kills.get(LOW_SWAP).copied().unwrap_or(0)
}

/// Whether the jetsam happened within 15 minutes before the panic.
fn precedes(j: &Jetsam, p: &Panic) -> bool {
    (0..=JETSAM_BEFORE_PANIC_SECS).contains(&(p.time - j.time))
}

/// The sample with the least disk free within ±10 minutes of the jetsam,
/// if that is below the disk warning threshold.
fn disk_nearly_full<'a>(input: &'a ReportInput, j: &Jetsam) -> Option<&'a HistorySample> {
    let warn = input.config.disk.warn_free_gb * GB;
    input
        .history
        .iter()
        .filter(|s| (s.t - j.time).abs() <= DISK_CHECK_SECS)
        .min_by_key(|s| s.disk_free)
        .filter(|s| (s.disk_free as f64) < warn)
}

/// History samples within `secs` before now (history is sorted by time).
fn in_window(input: &ReportInput, secs: i64) -> &[HistorySample] {
    let h = &input.history;
    let start = h.partition_point(|s| s.t < input.now - secs);
    let end = h.partition_point(|s| s.t <= input.now);
    &h[start..end.max(start)]
}

/// The last uninterrupted run of `samples`, if it reaches up to now (no gap
/// before now) and covers at least 30 minutes.
fn current_run<'a>(
    input: &ReportInput,
    samples: &'a [HistorySample],
) -> Option<&'a [HistorySample]> {
    let interval = input.config.history.interval_secs;
    let max_gap = i64::try_from(interval.saturating_mul(3)).unwrap_or(i64::MAX);
    let run = segments(samples, interval).pop()?;
    let (first, last) = (run.first()?, run.last()?);
    let current = input.now.saturating_sub(last.t) <= max_gap;
    (current && last.t - first.t >= MIN_COVERAGE_SECS).then_some(run)
}

/// "; if this continues, <subject> would likely fall below 10.0 GB free in about 1 h 33 min",
/// or a note that it already is below, or nothing for far-off projections.
fn disk_projection(disk_now: f64, fall_per_h: f64, red: f64, subject: &str) -> String {
    if disk_now < red {
        return format!("; disk free is already below {}", fmt_gb(red as u64));
    }
    match hours_until(disk_now, -fall_per_h, red) {
        Some(h) if h <= MAX_PROJECTION_H => format!(
            "; if this continues, {subject} would likely fall below {} free in about {}",
            fmt_gb(red as u64),
            fmt_duration((h * 3600.0) as i64)
        ),
        _ => String::new(),
    }
}

/// Memory and swap are not OK, so the current top apps are worth naming.
fn struggling(input: &ReportInput) -> bool {
    let e = &input.eval;
    [e.memory, e.swap, e.pressure]
        .iter()
        .any(|s| *s != Status::Ok)
}

fn when(t: i64, now: i64) -> String {
    fmt_relative(t, now)
}

fn days(input: &ReportInput) -> String {
    let n = input.config.report.incident_days;
    count(usize::try_from(n).unwrap_or(usize::MAX), "day", "days")
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn counted(label: &str, n: usize) -> String {
    if n > 1 {
        format!("{label} (×{n})")
    } else {
        label.to_string()
    }
}

/// `120` from "... in 120 seconds".
fn seconds_in(text: &str) -> Option<u64> {
    let words: Vec<&str> = text.split_whitespace().collect();
    words.windows(3).find_map(|w| {
        (w[0] == "in" && w[2].starts_with("second"))
            .then(|| w[1].parse().ok())
            .flatten()
    })
}

fn truncate(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::Config;
    use crate::diag::{Incident, Jetsam, Panic, ProcGroup, Watchdog, display_name};
    use crate::eval::Evaluation;
    use crate::history::HistorySample;
    use crate::metrics::Snapshot;
    use crate::processes::Culprit;
    use crate::status::{PressureLevel, Status};

    const NOW: i64 = 1_795_080_000;
    const GIB_U: u64 = 1 << 30;
    const MIB_U: u64 = 1 << 20;

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

    fn joined(i: &ReportInput) -> String {
        findings(i).join("\n")
    }

    fn panic(time: i64, first_line: &str, hung: Option<&str>) -> Incident {
        Incident::Panic(Panic {
            time,
            time_is_exact: true,
            first_line: first_line.into(),
            hung_service: hung.map(str::to_string),
        })
    }

    fn group(name: &str, bytes: u64, count: usize) -> ProcGroup {
        ProcGroup {
            name: name.into(),
            label: display_name(name),
            bytes,
            count,
        }
    }

    fn jetsam(time: i64, kills: &[(&str, usize)], groups: Vec<ProcGroup>) -> Jetsam {
        Jetsam {
            time,
            free_bytes: 170 * MIB_U,
            compressor_bytes: 7 * GIB_U,
            largest_process: None,
            kills: kills
                .iter()
                .map(|(k, n)| (k.to_string(), *n))
                .collect::<BTreeMap<_, _>>(),
            per_process_limit: vec![],
            groups,
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
            top: vec![],
        }
    }

    const WATCHDOG_LINE: &str = "userspace watchdog timeout: no successful checkins from WindowServer (2 induced crashes) in 120 seconds";

    #[test]
    fn rule_panic_watchdog() {
        let mut i = input();
        i.incidents = vec![panic(NOW - 300, WATCHDOG_LINE, Some("WindowServer"))];
        let f = joined(&i);
        assert!(f.contains("froze and restarted itself"), "{f}");
        assert!(f.contains("WindowServer stopped responding"), "{f}");
        assert!(f.contains("120 s"), "{f}");
    }

    #[test]
    fn rule_panic_other_is_quoted() {
        let mut i = input();
        i.incidents = vec![panic(NOW - 300, "Kernel data abort", None)];
        let f = joined(&i);
        assert!(f.contains("\"Kernel data abort\""), "{f}");
        assert!(!f.contains("froze and restarted"), "{f}");
    }

    #[test]
    fn rule_panic_inexact_time_is_labelled() {
        let mut i = input();
        let Incident::Panic(mut p) = panic(NOW - 300, "boom", None) else {
            unreachable!()
        };
        p.time_is_exact = false;
        i.incidents = vec![Incident::Panic(p)];
        assert!(joined(&i).contains("reported at"));
    }

    #[test]
    fn rule_watchdog_without_panic() {
        let mut i = input();
        let w = Incident::Watchdog(Watchdog {
            time: NOW - 900,
            service: Some("configd".into()),
            reason: "checkin with service: configd returned not alive".into(),
        });
        i.incidents = vec![w.clone()];
        let f = joined(&i);
        assert!(f.contains("configd was unresponsive"), "{f}");

        // A panic shortly after means the watchdog is part of that panic.
        i.incidents = vec![w, panic(NOW - 700, WATCHDOG_LINE, Some("WindowServer"))];
        assert!(!joined(&i).contains("configd was unresponsive"));
    }

    #[test]
    fn rule_memory_before_panic() {
        let mut i = input();
        let p = NOW - 300;
        i.incidents = vec![
            Incident::Jetsam(jetsam(
                p - 300,
                &[("low-swap", 102), ("idle-exit", 32)],
                vec![],
            )),
            panic(p, WATCHDOG_LINE, Some("WindowServer")),
        ];
        let f = joined(&i);
        assert!(f.contains("memory ran out first"), "{f}");
        assert!(f.contains("102"), "{f}");
        assert!(f.contains("170 MB"), "{f}");

        i.incidents = vec![
            Incident::Jetsam(jetsam(p - 1200, &[("low-swap", 102)], vec![])),
            panic(p, WATCHDOG_LINE, Some("WindowServer")),
        ];
        assert!(!joined(&i).contains("memory ran out first"));
    }

    #[test]
    fn rule_swap_exhausted_disk_full() {
        let mut i = input();
        let jt = NOW - 1800;
        i.incidents = vec![Incident::Jetsam(jetsam(jt, &[("low-swap", 5)], vec![]))];
        let around = |disk: u64| -> Vec<HistorySample> {
            (0..=30)
                .map(|k| HistorySample {
                    disk_free: disk,
                    ..sample(jt - 900 + 60 * k)
                })
                .collect()
        };
        i.history = around(9_100_000_000);
        let f = joined(&i);
        assert!(f.contains("likely because the disk was nearly full"), "{f}");
        assert!(f.contains("9.1 GB"), "{f}");

        i.history = around(100_000_000_000);
        let f = joined(&i);
        assert!(!f.contains("nearly full"), "{f}");
        assert!(f.contains("low-swap"), "{f}");
    }

    #[test]
    fn spec_example_in_rule_order() {
        let mut i = input();
        let p = NOW - 540;
        let mut j = jetsam(
            p - 240,
            &[
                ("low-swap", 102),
                ("idle-exit", 32),
                ("per-process-limit", 1),
            ],
            vec![
                group("plugin-container", 21 * GIB_U / 2, 43),
                group("com.apple.Virtualization.Virtual", 72 * GIB_U / 10, 1),
                group("phpstorm", 39 * GIB_U / 10, 1),
                group("sysinfod", 5 * GIB_U / 2, 1),
            ],
        );
        j.per_process_limit = vec!["phpstorm".into()];
        i.incidents = vec![
            Incident::Jetsam(j),
            panic(p, WATCHDOG_LINE, Some("WindowServer")),
        ];
        i.history = (0..=20)
            .map(|k| HistorySample {
                disk_free: 9_100_000_000,
                ..sample(p - 240 - 1200 + 60 * k)
            })
            .collect();
        let f = findings(&i);
        let pos = |needle: &str| {
            f.iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle} missing from {f:?}"))
        };
        let order = [
            pos("froze and restarted itself"),
            pos("memory ran out first"),
            pos("nearly full"),
            pos("hit its own memory limit"),
            pos("Biggest memory users"),
            pos("unusual"),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{f:?}");
        assert_eq!(f.len(), 6, "{f:?}");
    }

    #[test]
    fn rule_per_process_limit() {
        let mut i = input();
        let mut j = jetsam(NOW - 600, &[("per-process-limit", 2)], vec![]);
        j.per_process_limit = vec!["WindowServer".into(), "WindowServer".into()];
        i.incidents = vec![Incident::Jetsam(j)];
        let f = joined(&i);
        assert!(f.contains("WindowServer hit its own memory limit"), "{f}");
        assert_eq!(f.matches("hit its own memory limit").count(), 1, "{f}");
    }

    #[test]
    fn rule_biggest_users_share() {
        let mut i = input();
        i.incidents = vec![Incident::Jetsam(jetsam(
            NOW - 600,
            &[],
            vec![
                group("plugin-container", 7 * GIB_U, 43),
                group("phpstorm", 2 * GIB_U, 1),
                group("node", GIB_U, 3),
                group("zsh", MIB_U, 1),
            ],
        ))];
        let f = joined(&i);
        assert!(f.contains("Firefox/Zen content processes (×43)"), "{f}");
        assert!(f.contains("%"), "{f}");
        assert!(f.contains("more than 30%"), "{f}");
        assert!(!f.contains("zsh"), "{f}");
    }

    #[test]
    fn rule_biggest_users_now_only_when_struggling() {
        let mut i = input();
        i.apps = vec![Culprit {
            label: "Zen".into(),
            bytes: 4 * GIB_U,
            count: 1,
        }];
        assert!(!joined(&i).contains("Zen"));
        i.eval.memory = Status::Warning;
        let f = joined(&i);
        assert!(f.contains("Zen 4.0 GB"), "{f}");
        assert!(!f.contains("more than 30%"), "{f}");
    }

    #[test]
    fn rule_system_service() {
        let mut i = input();
        i.incidents = vec![Incident::Jetsam(jetsam(
            NOW - 600,
            &[],
            vec![group("sysinfod", 5 * GIB_U / 2, 1)],
        ))];
        let f = joined(&i);
        assert!(f.contains("sysinfod"), "{f}");
        assert!(f.contains("unusual"), "{f}");

        i.incidents = vec![Incident::Jetsam(jetsam(
            NOW - 600,
            &[],
            vec![group("WindowServer", 2 * GIB_U, 1)],
        ))];
        assert!(!joined(&i).contains("unusual"));
    }

    #[test]
    fn rule_system_service_now() {
        let mut i = input();
        i.procs = vec![ProcInfo {
            pid: 9,
            path: "/usr/libexec/sysinfod".into(),
            argv0: "/usr/libexec/sysinfod".into(),
            footprint_bytes: 3 * GIB_U,
        }];
        let f = joined(&i);
        assert!(f.contains("sysinfod"), "{f}");
        assert!(f.contains("unusual"), "{f}");
    }

    #[test]
    fn system_services_filters() {
        let p = |path: &str, bytes: u64| ProcInfo {
            pid: 1,
            path: path.into(),
            argv0: String::new(),
            footprint_bytes: bytes,
        };
        let procs = [
            p("/usr/libexec/sysinfod", 3 * GIB_U),
            p("/usr/sbin/cfprefsd", 2 * GIB_U),
            p(
                "/System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer",
                4 * GIB_U,
            ),
            p(
                "/System/Library/Frameworks/Virtualization.framework/Versions/A/XPCServices/com.apple.Virtualization.VirtualMachine.xpc/Contents/MacOS/com.apple.Virtualization.VirtualMachine",
                8 * GIB_U,
            ),
            p(
                "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent",
                3 * GIB_U,
            ),
            p(
                "/System/Applications/Mail.app/Contents/MacOS/Mail",
                2 * GIB_U,
            ),
            p("/Applications/Zen.app/Contents/MacOS/zen", 9 * GIB_U),
            p("/usr/libexec/logd", GIB_U),
        ];
        assert_eq!(
            system_services(&procs, GIB_U),
            vec![
                ("sysinfod".to_string(), 3 * GIB_U),
                ("cfprefsd".to_string(), 2 * GIB_U)
            ]
        );
    }

    #[test]
    fn system_service_names() {
        for n in [
            "sysinfod",
            "mds_stores",
            "mdworker",
            "cloudd",
            "com.apple.geod",
        ] {
            assert!(is_system_service_name(n), "{n}");
        }
        for n in [
            "WindowServer",
            "kernel_task",
            "com.apple.Virtualization.Virtual",
            "com.apple.WebKit.WebContent",
            "plugin-container",
            "node",
        ] {
            assert!(!is_system_service_name(n), "{n}");
        }
    }

    fn swap_ramp(minutes: i64, from: u64, to: u64) -> Vec<HistorySample> {
        (0..=minutes)
            .map(|k| HistorySample {
                swap_used: from + (to - from) * k as u64 / minutes as u64,
                ..sample(NOW - 60 * (minutes - k))
            })
            .collect()
    }

    #[test]
    fn rule_swap_trend() {
        let mut i = input();
        i.history = swap_ramp(60, 10 * GIB_U, 13 * GIB_U);
        let f = joined(&i);
        assert!(f.contains("Swap has grown"), "{f}");
        assert!(f.contains("3.0 GB"), "{f}");

        i.history = swap_ramp(20, 10 * GIB_U, 13 * GIB_U);
        assert!(!joined(&i).contains("Swap has grown"));
    }

    #[test]
    fn rule_swap_trend_projects_disk() {
        let mut i = input();
        // 6 GiB/h of swap growth against 20 GB free, red at 10 GB: about 1.55 h.
        i.snapshot.disk_free_bytes = 20_000_000_000;
        i.history = swap_ramp(60, 10 * GIB_U, 16 * GIB_U);
        let f = joined(&i);
        assert!(f.contains("below 10.0 GB"), "{f}");
        assert!(f.contains("1 h 33 min"), "{f}");
    }

    #[test]
    fn rule_disk_falling() {
        let mut i = input();
        i.history = (0..=40)
            .map(|k| HistorySample {
                disk_free: 60_000_000_000 - 200_000_000 * k as u64,
                ..sample(NOW - 60 * (40 - k))
            })
            .collect();
        let f = joined(&i);
        assert!(f.contains("Disk free space has fallen 8.0 GB"), "{f}");
    }

    fn red_run(times: impl Iterator<Item = i64>) -> Vec<HistorySample> {
        times
            .map(|t| HistorySample {
                swap_status: 2,
                swap_used: 12 * GIB_U,
                ..sample(t)
            })
            .collect()
    }

    #[test]
    fn rule_time_to_restart() {
        let mut i = input();
        i.history = red_run((0..31).map(|k| NOW - 1800 + 60 * k));
        let f = joined(&i);
        assert!(f.contains("a restart would likely help"), "{f}");
        assert!(f.contains("uptime"), "{f}");

        // One 10-minute gap: the last uninterrupted run is only 15 minutes.
        i.history = red_run((0..31).map(|k| NOW - 2400 + 60 * k + if k >= 15 { 600 } else { 0 }));
        assert!(!joined(&i).contains("restart"));
    }

    #[test]
    fn rule_time_to_restart_long_uptime() {
        let mut i = input();
        i.boot_time = Some(NOW - 8 * 86_400);
        assert!(!joined(&i).contains("restart"));
        i.eval.swap = Status::Warning;
        let f = joined(&i);
        assert!(f.contains("a restart would likely help"), "{f}");
        assert!(f.contains("8 d 0 h"), "{f}");
    }

    #[test]
    fn rule_nothing_found() {
        assert_eq!(
            findings(&input()),
            vec!["No incidents in the last 7 days and nothing unusual right now.".to_string()]
        );
    }

    #[test]
    fn rule_nothing_found_not_claimed_when_red() {
        let mut i = input();
        i.eval.disk = Status::Critical;
        let f = findings(&i);
        assert_eq!(f, vec!["No incidents in the last 7 days.".to_string()]);
    }

    #[test]
    fn hours_until_projection() {
        assert_eq!(hours_until(20.0, -5.0, 10.0), Some(2.0));
        assert_eq!(hours_until(20.0, 5.0, 10.0), None);
        assert_eq!(hours_until(20.0, 0.0, 10.0), None);
        assert_eq!(hours_until(5.0, -5.0, 10.0), None);
    }

    #[test]
    fn fmt_mem_units() {
        assert_eq!(fmt_mem(170 * MIB_U), "170 MB");
        assert_eq!(fmt_mem(5 * GIB_U / 2), "2.5 GB");
    }
}
