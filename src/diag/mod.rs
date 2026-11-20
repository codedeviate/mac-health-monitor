//! Parsers for macOS diagnostic reports (kernel panic, userspace watchdog,
//! JetsamEvent, ResetCounter). The formats are undocumented, so every parser
//! is tolerant: odd or missing data yields `Err` or a fallback, never a panic.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::clock::parse_apple_date;

/// One diagnostic incident, parsed from a report file.
#[derive(Debug, Clone, PartialEq)]
pub enum Incident {
    Panic(Panic),
    Watchdog(Watchdog),
    Jetsam(Jetsam),
    Reset(Reset),
}

impl Incident {
    /// Unix seconds at which the incident happened.
    pub fn time(&self) -> i64 {
        match self {
            Incident::Panic(p) => p.time,
            Incident::Watchdog(w) => w.time,
            Incident::Jetsam(j) => j.time,
            Incident::Reset(r) => r.time,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Panic {
    pub time: i64,
    /// False when `time` is the file's timestamp rather than one from the body.
    pub time_is_exact: bool,
    /// Panic string without the `panic(cpu N caller 0x…): ` prefix.
    pub first_line: String,
    /// Service named in "no successful checkins from <service>".
    pub hung_service: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Watchdog {
    pub time: i64,
    pub service: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reset {
    pub time: i64,
    pub boot_faults: String,
}

impl Reset {
    pub fn is_watchdog(&self) -> bool {
        self.boot_faults.contains("wdog")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcGroup {
    /// Raw process name.
    pub name: String,
    /// Display label.
    pub label: String,
    pub bytes: u64,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Jetsam {
    pub time: i64,
    pub free_bytes: u64,
    pub compressor_bytes: u64,
    pub largest_process: Option<String>,
    /// Kill reason -> number of processes.
    pub kills: BTreeMap<String, usize>,
    /// Names of processes killed for exceeding their own limit.
    pub per_process_limit: Vec<String>,
    /// All processes grouped by raw name, largest first.
    pub groups: Vec<ProcGroup>,
}

/// Human-friendly label for a raw process name.
pub fn display_name(raw: &str) -> String {
    if raw == "plugin-container" {
        "Firefox/Zen content processes".to_string()
    } else if raw.starts_with("com.apple.Virtualization.Virtual") {
        "Virtual machine (Docker)".to_string()
    } else if raw.contains('.') && raw.chars().all(|c| c.is_ascii_digit() || c == '.') {
        "Claude Code".to_string()
    } else {
        raw.to_string()
    }
}

/// Splits `.ips`/`.panic` text into (header line, body).
fn split_header(text: &str) -> Result<(&str, &str), String> {
    text.split_once('\n')
        .ok_or_else(|| "no body after header line".to_string())
}

#[derive(Deserialize)]
struct Header {
    timestamp: Option<String>,
}

#[derive(Deserialize)]
struct PanicBody {
    #[serde(rename = "panicString")]
    panic_string: Option<String>,
    #[serde(rename = "panicTimeInSecsAndUsecs")]
    panic_time: Option<String>,
}

/// Parses `"wall 1795074978 s 893998 us"`.
fn parse_wall(s: &str) -> Option<i64> {
    let mut it = s.split_whitespace();
    if it.next()? != "wall" {
        return None;
    }
    it.next()?.parse().ok()
}

/// Finds `Calendar: 0x6afeaba2 0x…` and returns the first value.
fn parse_calendar(panic_string: &str) -> Option<i64> {
    let rest = panic_string.split("Calendar: 0x").nth(1)?;
    let hex: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    i64::from_str_radix(&hex, 16).ok()
}

pub fn parse_panic(text: &str, file_time: i64) -> Result<Panic, String> {
    let (_, body) = split_header(text)?;
    let body: PanicBody =
        serde_json::from_str(body).map_err(|e| format!("panic body is not valid JSON: {e}"))?;
    let panic_string = body.panic_string.ok_or("panic body has no panicString")?;

    let wall = body.panic_time.as_deref().and_then(parse_wall);
    let (time, time_is_exact) = match wall.or_else(|| parse_calendar(&panic_string)) {
        Some(t) => (t, true),
        None => (file_time, false),
    };

    let line = panic_string.lines().next().unwrap_or("");
    let first_line = match line.strip_prefix("panic(") {
        Some(rest) => rest.split_once("): ").map_or(line, |(_, msg)| msg),
        None => line,
    }
    .to_string();

    let hung_service = first_line
        .split("no successful checkins from ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .map(str::to_string);

    Ok(Panic {
        time,
        time_is_exact,
        first_line,
        hung_service,
    })
}

#[derive(Deserialize)]
struct JetsamBody {
    date: Option<String>,
    #[serde(rename = "memoryStatus")]
    memory_status: Option<MemoryStatus>,
    #[serde(rename = "largestProcess")]
    largest_process: Option<String>,
    processes: Option<Vec<JetsamProcess>>,
}

#[derive(Deserialize)]
struct MemoryStatus {
    #[serde(rename = "pageSize")]
    page_size: Option<u64>,
    #[serde(rename = "compressorSize")]
    compressor_size: Option<u64>,
    #[serde(rename = "memoryPages")]
    memory_pages: Option<MemoryPages>,
}

#[derive(Deserialize)]
struct MemoryPages {
    free: Option<u64>,
}

#[derive(Deserialize)]
struct JetsamProcess {
    name: Option<String>,
    rpages: Option<u64>,
    reason: Option<String>,
}

pub fn parse_jetsam(text: &str) -> Result<Jetsam, String> {
    let (header, body) = split_header(text)?;
    let body: JetsamBody =
        serde_json::from_str(body).map_err(|e| format!("jetsam body is not valid JSON: {e}"))?;

    let time = body
        .date
        .as_deref()
        .and_then(parse_apple_date)
        .or_else(|| {
            serde_json::from_str::<Header>(header)
                .ok()
                .and_then(|h| h.timestamp)
                .as_deref()
                .and_then(parse_apple_date)
        })
        .ok_or("jetsam report has no usable date")?;

    let ms = body
        .memory_status
        .ok_or("jetsam report has no memoryStatus")?;
    let page_size = ms.page_size.ok_or("jetsam report has no pageSize")?;
    let free_pages = ms
        .memory_pages
        .and_then(|m| m.free)
        .ok_or("jetsam report has no free page count")?;

    let mut kills: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_process_limit = Vec::new();
    let mut by_name: BTreeMap<String, (u64, usize)> = BTreeMap::new();
    for p in body.processes.unwrap_or_default() {
        let name = p.name.unwrap_or_else(|| "?".to_string());
        let entry = by_name.entry(name.clone()).or_default();
        entry.0 = entry
            .0
            .saturating_add(p.rpages.unwrap_or(0).saturating_mul(page_size));
        entry.1 += 1;
        if let Some(reason) = p.reason {
            if reason == "per-process-limit" {
                per_process_limit.push(name);
            }
            *kills.entry(reason).or_default() += 1;
        }
    }
    let mut groups: Vec<ProcGroup> = by_name
        .into_iter()
        .map(|(name, (bytes, count))| ProcGroup {
            label: display_name(&name),
            name,
            bytes,
            count,
        })
        .collect();
    groups.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));

    Ok(Jetsam {
        time,
        free_bytes: free_pages.saturating_mul(page_size),
        compressor_bytes: ms.compressor_size.unwrap_or(0).saturating_mul(page_size),
        largest_process: body.largest_process,
        kills,
        per_process_limit,
        groups,
    })
}

/// Value of the first line starting with `key` (after the key, trimmed).
fn line_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|l| l.strip_prefix(key))
        .map(str::trim)
}

pub fn parse_spin(text: &str) -> Result<Watchdog, String> {
    let time = line_value(text, "Date/Time:")
        .and_then(parse_apple_date)
        .ok_or("spin report has no usable Date/Time")?;
    let reason = line_value(text, "Reason:").ok_or("spin report has no Reason")?;
    let service = reason
        .split("checkin with service: ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .map(str::to_string);
    Ok(Watchdog {
        time,
        service,
        reason: reason.to_string(),
    })
}

pub fn parse_reset(text: &str) -> Result<Reset, String> {
    let time = line_value(text, "Date:")
        .and_then(parse_apple_date)
        .ok_or("reset report has no usable Date")?;
    let boot_faults = line_value(text, "Boot faults:").ok_or("reset report has no Boot faults")?;
    Ok(Reset {
        time,
        boot_faults: boot_faults.to_string(),
    })
}

#[cfg(test)]
pub(crate) mod fixtures {
    use serde_json::json;

    /// `.ips`-style text: one-line JSON header, newline, JSON body.
    pub fn ips_text(body: &serde_json::Value) -> String {
        format!("{{\"bug_type\":\"298\"}}\n{body}")
    }

    /// Kernel panic report. `wall` is the `panicTimeInSecsAndUsecs` value.
    pub fn panic_text(wall: Option<&str>, panic_string: &str) -> String {
        let mut body = json!({ "panicString": panic_string });
        if let Some(w) = wall {
            body["panicTimeInSecsAndUsecs"] = json!(w);
        }
        ips_text(&body)
    }

    /// `procs` are `(name, rpages, reason)`.
    pub fn jetsam_text(
        date: &str,
        page_size: u64,
        free_pages: u64,
        compressor_pages: u64,
        largest: Option<&str>,
        procs: &[(&str, u64, Option<&str>)],
    ) -> String {
        let processes: Vec<_> = procs
            .iter()
            .map(|(name, rpages, reason)| {
                let mut p = json!({ "name": name, "rpages": rpages });
                if let Some(r) = reason {
                    p["reason"] = json!(r);
                }
                p
            })
            .collect();
        let mut body = json!({
            "date": date,
            "memoryStatus": {
                "pageSize": page_size,
                "compressorSize": compressor_pages,
                "memoryPages": { "free": free_pages },
            },
            "processes": processes,
        });
        if let Some(l) = largest {
            body["largestProcess"] = json!(l);
        }
        ips_text(&body)
    }

    /// Watchdog `.spin` text.
    pub fn spin_text(date: &str, reason: &str) -> String {
        format!(
            "Date/Time:        {date}\nEnd time:         {date}\nReason:           {reason}\nCommand:          x\n"
        )
    }

    /// `ResetCounter` text.
    pub fn reset_text(date: &str, boot_faults: &str) -> String {
        format!("Incident Identifier: X\nDate: {date}\nBoot faults: {boot_faults}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANIC_LINE: &str = "panic(cpu 0 caller 0xfffffe004aac0b30): userspace watchdog timeout: no successful checkins from WindowServer (2 induced crashes) in 120 seconds\nservice: logd …";

    #[test]
    fn parse_panic_wall_time() {
        let text = fixtures::panic_text(Some("wall 1795074978 s 893998 us"), PANIC_LINE);
        let p = parse_panic(&text, 5).unwrap();
        assert_eq!(p.time, 1795074978);
        assert!(p.time_is_exact);
        assert_eq!(
            p.first_line,
            "userspace watchdog timeout: no successful checkins from WindowServer (2 induced crashes) in 120 seconds"
        );
        assert_eq!(p.hung_service.as_deref(), Some("WindowServer"));
    }

    #[test]
    fn parse_panic_calendar_fallback() {
        let s = format!("{PANIC_LINE}\n  Calendar: 0x6afeaba2 0x000da462\n");
        let p = parse_panic(&fixtures::panic_text(None, &s), 5).unwrap();
        assert_eq!(p.time, 0x6afeaba2);
        assert!(p.time_is_exact);
    }

    #[test]
    fn parse_panic_missing_time_falls_back() {
        let p = parse_panic(
            &fixtures::panic_text(None, "panic(cpu 1 caller 0x1): boom"),
            42,
        )
        .unwrap();
        assert_eq!(p.time, 42);
        assert!(!p.time_is_exact);
        assert_eq!(p.first_line, "boom");
        assert_eq!(p.hung_service, None);
    }

    #[test]
    fn parse_panic_rejects_garbage() {
        assert!(parse_panic("", 1).is_err());
        assert!(parse_panic("{}\n{\"panicStr", 1).is_err());
        assert!(parse_panic("{}\n{}", 1).is_err());
    }

    #[test]
    fn parse_jetsam_basic() {
        let procs = [
            ("plugin-container", 1000, None),
            ("plugin-container", 3000, None),
            ("2.1.325", 500, None),
            ("phpstorm", 10, Some("per-process-limit")),
            ("x", 1, Some("low-swap")),
            ("x", 1, Some("low-swap")),
            ("x", 1, Some("low-swap")),
        ];
        let text = fixtures::jetsam_text(
            "2026-11-19 08:52:29.95 +0100",
            16384,
            10959,
            100,
            Some("com.apple.Virtualization.Virtual"),
            &procs,
        );
        let j = parse_jetsam(&text).unwrap();
        assert_eq!(
            j.time,
            parse_apple_date("2026-11-19 08:52:29.95 +0100").unwrap()
        );
        assert_eq!(j.free_bytes, 10959 * 16384);
        assert_eq!(j.compressor_bytes, 100 * 16384);
        assert_eq!(
            j.largest_process.as_deref(),
            Some("com.apple.Virtualization.Virtual")
        );
        let kills: BTreeMap<String, usize> = [("low-swap", 3), ("per-process-limit", 1)]
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect();
        assert_eq!(j.kills, kills);
        assert_eq!(j.per_process_limit, vec!["phpstorm".to_string()]);
        assert_eq!(
            j.groups[0],
            ProcGroup {
                name: "plugin-container".into(),
                label: "Firefox/Zen content processes".into(),
                bytes: 4000 * 16384,
                count: 2
            }
        );
        let claude = j.groups.iter().find(|g| g.name == "2.1.325").unwrap();
        assert_eq!(claude.label, "Claude Code");
        assert!(j.groups.windows(2).all(|w| w[0].bytes >= w[1].bytes));
    }

    #[test]
    fn parse_jetsam_rejects_truncated() {
        assert!(parse_jetsam("{\"bug_type\":\"298\"}\n{\"memoryStatus\": {").is_err());
        assert!(parse_jetsam("").is_err());
    }

    #[test]
    fn parse_spin_reason() {
        let text = fixtures::spin_text(
            "2026-11-19 08:59:47.664 +0100",
            "(1 monitored services unresponsive): checkin with service: configd (0 induced crashes) returned not alive",
        );
        let w = parse_spin(&text).unwrap();
        assert_eq!(w.service.as_deref(), Some("configd"));
        assert_eq!(
            Some(w.time),
            parse_apple_date("2026-11-19 08:59:47.664 +0100")
        );
        assert!(w.reason.contains("monitored services"));
        assert!(parse_spin("nothing useful").is_err());
    }

    #[test]
    fn parse_reset_wdog() {
        let r = parse_reset(&fixtures::reset_text(
            "2026-11-19 08:57:50.87 +0100",
            "wdog,reset_in_1",
        ))
        .unwrap();
        assert!(r.is_watchdog());
        assert_eq!(r.boot_faults, "wdog,reset_in_1");
        let r = parse_reset(&fixtures::reset_text(
            "2026-11-19 08:57:50.87 +0100",
            "none",
        ))
        .unwrap();
        assert!(!r.is_watchdog());
        assert!(parse_reset("Boot faults: wdog").is_err());
    }

    #[test]
    fn display_names() {
        assert_eq!(
            display_name("plugin-container"),
            "Firefox/Zen content processes"
        );
        assert_eq!(
            display_name("com.apple.Virtualization.Virtual"),
            "Virtual machine (Docker)"
        );
        assert_eq!(display_name("2.1.325"), "Claude Code");
        assert_eq!(display_name("1234"), "1234");
        assert_eq!(display_name("node"), "node");
    }

    #[test]
    fn incident_time() {
        let r = Incident::Reset(Reset {
            time: 7,
            boot_faults: String::new(),
        });
        assert_eq!(r.time(), 7);
    }
}
