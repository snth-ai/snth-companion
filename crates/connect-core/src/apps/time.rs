//! Points in time as the synth writes them, and the computer's own clock.
//!
//! A time is either absolute (RFC 3339 with `Z` or an offset) or local
//! ("2026-10-13T15:00", "2026-10-13"): local means the clock of THIS
//! computer, which is what a person means by "tomorrow at three". Calendar
//! and Reminders work in local time, so both forms end up as local date
//! parts. Results go back as RFC 3339 with this computer's offset.
//!
//! No date crate: the conversions are `localtime_r` / `mktime` on Unix.
//! On Windows (where no app needs them yet) absolute times are shown in
//! UTC.

use super::AppError;

/// Date and time parts on this computer's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Local {
    /// "2026-10-13 15:00", for the owner's eyes.
    pub fn display(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }

    /// Seconds since 1970 as if these parts were UTC. Good for ordering
    /// two local times and nothing else.
    pub fn ordinal(&self) -> i64 {
        days_from_civil(self.year as i64, self.month, self.day) * 86_400
            + self.hour as i64 * 3600
            + self.minute as i64 * 60
            + self.second as i64
    }
}

/// A time from the synth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// Unix seconds (the text had `Z` or an offset).
    At(i64),
    /// Already on this computer's clock (no offset in the text).
    Local(Local),
}

impl When {
    /// Parse "YYYY-MM-DD", "YYYY-MM-DDTHH:MM", "YYYY-MM-DDTHH:MM:SS", each
    /// of the last two optionally followed by `Z` or `+HH:MM` / `-HH:MM`.
    /// A space instead of `T` is accepted.
    pub fn parse(field: &str, text: &str) -> Result<When, AppError> {
        let bad = || {
            AppError::BadRequest(format!(
                "{field} must look like 2026-10-13T15:00 (this computer's time) or 2026-10-13T15:00:00+03:00"
            ))
        };
        let t = text.trim();
        let b = t.as_bytes();
        if b.len() < 10 || !t.is_ascii() {
            return Err(bad());
        }
        let num = |from: usize, len: usize| -> Option<u32> {
            let part = t.get(from..from + len)?;
            if part.bytes().all(|c| c.is_ascii_digit()) {
                part.parse().ok()
            } else {
                None
            }
        };
        if b[4] != b'-' || b[7] != b'-' {
            return Err(bad());
        }
        let (year, month, day) = match (num(0, 4), num(5, 2), num(8, 2)) {
            (Some(y), Some(m), Some(d)) => (y as i32, m, d),
            _ => return Err(bad()),
        };
        if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) || year < 1971 {
            return Err(bad());
        }
        let mut local = Local { year, month, day, hour: 0, minute: 0, second: 0 };
        if b.len() == 10 {
            return Ok(When::Local(local));
        }
        if (b[10] != b'T' && b[10] != b' ' && b[10] != b't') || b.len() < 16 || b[13] != b':' {
            return Err(bad());
        }
        let (hour, minute) = match (num(11, 2), num(14, 2)) {
            (Some(h), Some(m)) if h < 24 && m < 60 => (h, m),
            _ => return Err(bad()),
        };
        local.hour = hour;
        local.minute = minute;
        let mut at = 16;
        if b.len() > at && b[at] == b':' {
            match num(at + 1, 2) {
                Some(s) if s < 60 => local.second = s,
                _ => return Err(bad()),
            }
            at += 3;
            // Fractions of a second are accepted and dropped.
            if b.len() > at && b[at] == b'.' {
                at += 1;
                while b.len() > at && b[at].is_ascii_digit() {
                    at += 1;
                }
            }
        }
        let zone = &t[at..];
        if zone.is_empty() {
            return Ok(When::Local(local));
        }
        let offset = if zone == "Z" || zone == "z" {
            0
        } else {
            let z = zone.as_bytes();
            if z.len() != 6 || (z[0] != b'+' && z[0] != b'-') || z[3] != b':' {
                return Err(bad());
            }
            let hh: i64 = zone[1..3].parse().map_err(|_| bad())?;
            let mm: i64 = zone[4..6].parse().map_err(|_| bad())?;
            if hh > 14 || mm > 59 {
                return Err(bad());
            }
            let secs = hh * 3600 + mm * 60;
            if z[0] == b'-' {
                -secs
            } else {
                secs
            }
        };
        Ok(When::At(local.ordinal() - offset))
    }

    /// The parts on this computer's clock.
    pub fn local(&self) -> Local {
        match self {
            When::Local(l) => *l,
            When::At(unix) => local_from_unix(*unix),
        }
    }
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(y) => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

fn utc_parts(unix: i64) -> Local {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Local {
        year,
        month,
        day,
        hour: (secs / 3600) as u32,
        minute: ((secs % 3600) / 60) as u32,
        second: (secs % 60) as u32,
    }
}

/// RFC 3339 in UTC ("2026-10-13T12:00:00Z").
pub fn rfc3339_utc(unix: i64) -> String {
    let p = utc_parts(unix);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        p.year, p.month, p.day, p.hour, p.minute, p.second
    )
}

#[cfg(unix)]
mod sys {
    use super::Local;
    use std::os::raw::{c_char, c_int, c_long};

    // `struct tm` as macOS and Linux (glibc, musl) lay it out.
    #[repr(C)]
    struct Tm {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
        tm_gmtoff: c_long,
        tm_zone: *const c_char,
    }

    extern "C" {
        fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
        fn mktime(tm: *mut Tm) -> i64;
    }

    fn zero() -> Tm {
        Tm {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 0,
            tm_mon: 0,
            tm_year: 0,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
            tm_gmtoff: 0,
            tm_zone: std::ptr::null(),
        }
    }

    /// (local parts, seconds east of UTC) of a unix time.
    pub fn local(unix: i64) -> Option<(Local, i64)> {
        let mut tm = zero();
        let done = unsafe { localtime_r(&unix, &mut tm) };
        if done.is_null() {
            return None;
        }
        Some((
            Local {
                year: tm.tm_year + 1900,
                month: (tm.tm_mon + 1) as u32,
                day: tm.tm_mday as u32,
                hour: tm.tm_hour as u32,
                minute: tm.tm_min as u32,
                second: tm.tm_sec as u32,
            },
            tm.tm_gmtoff as i64,
        ))
    }

    /// The unix time of local parts (the system decides about DST).
    pub fn unix(l: &Local) -> Option<i64> {
        let mut tm = zero();
        tm.tm_year = l.year - 1900;
        tm.tm_mon = l.month as c_int - 1;
        tm.tm_mday = l.day as c_int;
        tm.tm_hour = l.hour as c_int;
        tm.tm_min = l.minute as c_int;
        tm.tm_sec = l.second as c_int;
        tm.tm_isdst = -1;
        let t = unsafe { mktime(&mut tm) };
        (t != -1).then_some(t)
    }
}

/// The parts of a unix time on this computer's clock (UTC where the
/// system offers no conversion).
pub fn local_from_unix(unix: i64) -> Local {
    #[cfg(unix)]
    if let Some((l, _)) = sys::local(unix) {
        return l;
    }
    utc_parts(unix)
}

/// Local parts as RFC 3339 with this computer's offset
/// ("2026-10-13T15:00:00+03:00"). Without a system conversion the parts
/// are returned with no offset.
pub fn rfc3339_local(l: &Local) -> String {
    let base = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        l.year, l.month, l.day, l.hour, l.minute, l.second
    );
    #[cfg(unix)]
    if let Some(off) = sys::unix(l).and_then(sys::local).map(|(_, off)| off) {
        let sign = if off < 0 { '-' } else { '+' };
        let off = off.abs();
        return format!("{base}{sign}{:02}:{:02}", off / 3600, (off % 3600) / 60);
    }
    base
}

/// Parse the "YYYY-MM-DDTHH:MM:SS" an AppleScript prints (local parts).
pub fn parse_local_parts(text: &str) -> Option<Local> {
    match When::parse("time", text).ok()? {
        When::Local(l) => Some(l),
        When::At(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_and_absolute_forms() {
        assert_eq!(
            When::parse("start", "2026-10-13").unwrap(),
            When::Local(Local { year: 2026, month: 10, day: 13, hour: 0, minute: 0, second: 0 })
        );
        assert_eq!(
            When::parse("start", "2026-10-13T15:30").unwrap(),
            When::Local(Local { year: 2026, month: 10, day: 13, hour: 15, minute: 30, second: 0 })
        );
        assert_eq!(
            When::parse("start", "2026-10-13 15:30:07").unwrap().local().second,
            7
        );
        // 2026-10-13T12:00:00Z
        let noon = 1_791_892_800;
        assert_eq!(When::parse("start", "2026-10-13T12:00:00Z").unwrap(), When::At(noon));
        assert_eq!(When::parse("start", "2026-10-13T15:00:00+03:00").unwrap(), When::At(noon));
        assert_eq!(When::parse("start", "2026-10-13T07:00:00.250-05:00").unwrap(), When::At(noon));
        assert_eq!(rfc3339_utc(noon), "2026-10-13T12:00:00Z");
    }

    #[test]
    fn refuses_what_is_not_a_time() {
        for bad in [
            "", "tomorrow", "2026-13-01", "2026-02-30", "2026-10-13T25:00", "2026-10-13T10:61",
            "2026-10-13T10:00+3", "13.10.2026", "2026-10-13T10:00:00+99:00", "1066-10-14",
            "2026-10-13T10:00; rm -rf",
        ] {
            assert!(When::parse("start", bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn leap_days() {
        assert!(When::parse("d", "2028-02-29").is_ok());
        assert!(When::parse("d", "2027-02-29").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn local_round_trip_on_this_clock() {
        // Whatever zone the machine is in, parts -> unix -> parts agree.
        let l = Local { year: 2026, month: 6, day: 15, hour: 12, minute: 0, second: 0 };
        let unix = sys::unix(&l).unwrap();
        assert_eq!(local_from_unix(unix), l);
        let text = rfc3339_local(&l);
        assert!(text.starts_with("2026-06-15T12:00:00"));
        assert_eq!(When::parse("x", &text).unwrap(), When::At(unix));
    }

    #[test]
    fn script_times_parse_back() {
        assert_eq!(parse_local_parts("2026-10-13T09:05:00").unwrap().display(), "2026-10-13 09:05");
        assert!(parse_local_parts("").is_none());
    }
}
