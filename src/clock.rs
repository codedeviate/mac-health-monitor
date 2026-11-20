//! Clock helpers: unix-second timestamps, boot time, and local-time formatting/parsing.

use std::ffi::CString;
use std::mem::{MaybeUninit, size_of};
use std::time::{SystemTime, UNIX_EPOCH};

/// Output formats for [`fmt_local`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// `2026-11-19 08:56:18`
    DateTime,
    /// `2026-11-19 08:56`
    DateHm,
    /// `08:56`
    Hm,
    /// `2026-11-19-085618`
    FileStamp,
}

/// Days since 1970-01-01 for a proleptic Gregorian civil date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Current time as unix seconds (0 if the clock is before the epoch).
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// Boot time as unix seconds, from `kern.boottime`.
pub fn boot_time() -> Option<i64> {
    let cname = CString::new("kern.boottime").ok()?;
    let mut value = MaybeUninit::<libc::timeval>::zeroed();
    let mut len = size_of::<libc::timeval>();
    // SAFETY: `cname` is NUL-terminated, `value` is a writable buffer of `len` bytes, and
    // `len` points to a valid usize; no new value is passed in.
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            value.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || len != size_of::<libc::timeval>() {
        return None;
    }
    // SAFETY: the kernel filled all `len == size_of::<timeval>()` bytes (and it was zeroed).
    let tv = unsafe { value.assume_init() };
    let secs = tv.tv_sec;
    (secs > 0).then_some(secs)
}

fn num(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Parses `"2026-11-19 08:52:29.95 +0100"` (fraction optional, dropped) to unix seconds.
/// Pure arithmetic; independent of the local timezone.
pub fn parse_apple_date(s: &str) -> Option<i64> {
    let mut parts = s.split_whitespace();
    let date = parts.next()?;
    let time = parts.next()?;
    let off = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let mut d = date.split('-');
    let (y, m, day) = (num(d.next()?)?, num(d.next()?)?, num(d.next()?)?);
    if d.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }

    let mut t = time.split(':');
    let (hh, mm) = (num(t.next()?)?, num(t.next()?)?);
    let sec_field = t.next()?;
    if t.next().is_some() {
        return None;
    }
    let (sec_str, frac) = match sec_field.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (sec_field, None),
    };
    if let Some(f) = frac {
        num(f)?;
    }
    let ss = num(sec_str)?;
    if hh > 23 || mm > 59 || ss > 60 {
        return None;
    }

    let sign = match off.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits = &off[1..];
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let off_secs = sign * (num(&digits[..2])? * 3600 + num(&digits[2..])? * 60);

    Some(days_from_civil(y, m, day) * 86_400 + hh * 3600 + mm * 60 + ss - off_secs)
}

/// Finds the first `YYYY-MM-DD-HHMMSS` in `s` and interprets it as local time.
pub fn parse_file_stamp(s: &str) -> Option<i64> {
    const LEN: usize = 17;
    let b = s.as_bytes();
    let shape_ok = |w: &[u8]| {
        w.iter().enumerate().all(|(i, c)| match i {
            4 | 7 | 10 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
    };
    let start = (0..b.len().checked_sub(LEN - 1)?).find(|&i| shape_ok(&b[i..i + LEN]))?;
    let w = &s[start..start + LEN];
    let field = |r: std::ops::Range<usize>| num(&w[r]).and_then(|v| i32::try_from(v).ok());

    // SAFETY: an all-zero `tm` is a valid value (null `tm_zone` is allowed).
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = field(0..4)? - 1900;
    tm.tm_mon = field(5..7)? - 1;
    tm.tm_mday = field(8..10)?;
    tm.tm_hour = field(11..13)?;
    tm.tm_min = field(13..15)?;
    tm.tm_sec = field(15..17)?;
    tm.tm_isdst = -1;
    if !(0..12).contains(&tm.tm_mon)
        || !(1..=31).contains(&tm.tm_mday)
        || tm.tm_hour > 23
        || tm.tm_min > 59
        || tm.tm_sec > 60
    {
        return None;
    }
    // SAFETY: `tm` is a valid, initialised struct that mktime may normalise in place.
    let t = unsafe { libc::mktime(&mut tm) };
    (t != -1).then_some(t)
}

fn local_tm(t: i64) -> Option<libc::tm> {
    let tt = libc::time_t::try_from(t).ok()?;
    // SAFETY: an all-zero `tm` is a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers refer to valid, live locals.
    let r = unsafe { libc::localtime_r(&tt, &mut tm) };
    (!r.is_null()).then_some(tm)
}

/// Formats unix seconds in local time. Falls back to the raw number if conversion fails.
pub fn fmt_local(t: i64, pattern: Pattern) -> String {
    let Some(tm) = local_tm(t) else {
        return t.to_string();
    };
    let (y, mo, d) = (tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday);
    let (h, mi, s) = (tm.tm_hour, tm.tm_min, tm.tm_sec);
    match pattern {
        Pattern::DateTime => format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}"),
        Pattern::DateHm => format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}"),
        Pattern::Hm => format!("{h:02}:{mi:02}"),
        Pattern::FileStamp => format!("{y:04}-{mo:02}-{d:02}-{h:02}{mi:02}{s:02}"),
    }
}

/// `"08:56 today"`, `"08:56 yesterday"`, otherwise `"2026-11-17 08:56"` (local calendar dates).
pub fn fmt_relative(t: i64, now: i64) -> String {
    let (Some(a), Some(b)) = (local_tm(t), local_tm(now)) else {
        return t.to_string();
    };
    let day = |tm: &libc::tm| {
        days_from_civil(
            i64::from(tm.tm_year) + 1900,
            i64::from(tm.tm_mon) + 1,
            i64::from(tm.tm_mday),
        )
    };
    match day(&b) - day(&a) {
        0 => format!("{} today", fmt_local(t, Pattern::Hm)),
        1 => format!("{} yesterday", fmt_local(t, Pattern::Hm)),
        _ => fmt_local(t, Pattern::DateHm),
    }
}

/// `"45 s"`, `"8 min"`, `"3 h 05 min"`, `"2 d 4 h"`.
pub fn fmt_duration(secs: i64) -> String {
    let secs = secs.max(0);
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m:02} min")
    } else if secs >= 60 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_apple_date_with_offset() {
        assert_eq!(
            parse_apple_date("2026-11-19 08:52:29.95 +0100"),
            Some(1795074749)
        );
        assert_eq!(
            parse_apple_date("2026-11-19 08:57:50 -0500"),
            Some(parse_apple_date("2026-11-19 13:57:50 +0000").unwrap())
        );
        assert_eq!(parse_apple_date("garbage"), None);
        assert_eq!(parse_apple_date("2026-11-19 08:52:29 +aéa"), None);
    }

    #[test]
    fn file_stamp_roundtrip() {
        let t = 1795074978;
        let name = format!("panic-full-{}.0002.panic", fmt_local(t, Pattern::FileStamp));
        assert_eq!(parse_file_stamp(&name), Some(t));
        assert!(
            parse_file_stamp("configd_2026-11-19-090111_Host.userspace_watchdog_timeout.spin")
                .is_some()
        );
        assert_eq!(parse_file_stamp("nostamp.ips"), None);
    }

    #[test]
    fn relative_today_yesterday() {
        let now = parse_file_stamp("x-2026-11-20-120000").unwrap();
        assert!(fmt_relative(now - 3600, now).ends_with("today"));
        assert!(fmt_relative(now - 86400, now).ends_with("yesterday"));
        assert!(fmt_relative(now - 3 * 86400, now).starts_with("2026-11-17"));
    }

    #[test]
    fn durations() {
        assert_eq!(fmt_duration(45), "45 s");
        assert_eq!(fmt_duration(480), "8 min");
        assert_eq!(fmt_duration(11100), "3 h 05 min");
        assert_eq!(fmt_duration(187200), "2 d 4 h");
    }

    #[test]
    fn boot_time_is_past() {
        assert!(boot_time().unwrap() < now_unix());
    }
}
