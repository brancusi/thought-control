//! Natural-language date parsing. Dates are stored as `YYYY-MM-DD` or floating local
//! `YYYY-MM-DDTHH:MM` strings, which sort lexically.

use anyhow::Result;
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Weekday};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateVal {
    Date(NaiveDate),
    DateTime(NaiveDateTime),
}

impl DateVal {
    pub fn date(&self) -> NaiveDate {
        match self {
            DateVal::Date(d) => *d,
            DateVal::DateTime(dt) => dt.date(),
        }
    }

    pub fn time(&self) -> Option<NaiveTime> {
        match self {
            DateVal::Date(_) => None,
            DateVal::DateTime(dt) => Some(dt.time()),
        }
    }

    pub fn with_date(&self, d: NaiveDate) -> DateVal {
        match self {
            DateVal::Date(_) => DateVal::Date(d),
            DateVal::DateTime(dt) => DateVal::DateTime(d.and_time(dt.time())),
        }
    }

    pub fn fmt(&self) -> String {
        match self {
            DateVal::Date(d) => d.format("%Y-%m-%d").to_string(),
            DateVal::DateTime(dt) => dt.format("%Y-%m-%dT%H:%M").to_string(),
        }
    }

    /// Parse a stored value.
    pub fn from_stored(s: &str) -> Option<DateVal> {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
            return Some(DateVal::DateTime(dt));
        }
        NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().map(DateVal::Date)
    }
}

/// `THC_NOW=2026-10-03T10:44` pins "now" for fixtures and tests (the dates a writer computes and
/// what "today" means). Event timestamps (HLC) still use the real clock.
/// A warning line when THC_NOW is set (doctor, prime, the TUI bar), so a leftover export can't
/// quietly mis-date real captures.
/// THC_FIXTURE_IDS (vault.rs `fixture_clock`) is louder still: it derives log ids from the pinned
/// clock and the device, so two devices could mint the same ones.
pub fn pinned_warning() -> Option<String> {
    if std::env::var("THC_FIXTURE_IDS").is_ok_and(|v| !v.is_empty()) {
        return Some("THC_FIXTURE_IDS is set: log ids are derived, not random · unset it (and THC_NOW) unless you're testing".into());
    }
    pinned_now().map(|n| format!("THC_NOW is set: \"now\" is pinned to {} · unset it unless you're testing", n.format("%a %b %-d %H:%M")))
}

fn pinned_now() -> Option<NaiveDateTime> {
    let v = std::env::var("THC_NOW").ok()?;
    NaiveDateTime::parse_from_str(&v, "%Y-%m-%dT%H:%M").ok()
}

pub fn today() -> NaiveDate {
    pinned_now().map(|n| n.date()).unwrap_or_else(|| Local::now().date_naive())
}

pub fn now_local() -> NaiveDateTime {
    pinned_now().unwrap_or_else(|| Local::now().naive_local())
}

pub fn now_stamp() -> String {
    now_local().format("%Y-%m-%dT%H:%M").to_string()
}

/// Parse user date input relative to `today`.
pub fn parse(input: &str, today: NaiveDate) -> Result<DateVal> {
    if let Some(v) = DateVal::from_stored(input.trim()) {
        return Ok(v);
    }
    let s = input.trim().to_lowercase().replace(['_'], " ");
    if s.is_empty() {
        return Err(crate::error::invalid("empty date"));
    }
    if let Some(v) = DateVal::from_stored(&s.replace('t', "T")) {
        return Ok(v);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M") {
        return Ok(DateVal::DateTime(dt));
    }
    // Stored values returned above; from here on it's what a person typed.
    reject_bare_m(&s)?;
    // `+2mo` / `-1mo`: months, spelled out.
    if let Some(num) = s.strip_suffix("mo") {
        if let Some(d) = parse_offset_days(&format!("{num}m"), today) {
            return Ok(DateVal::Date(d));
        }
    }
    // Relative hours/minutes from now: "+2h", "-1h", "+30min".
    for (suffix, mins) in [("min", 1i64), ("h", 60)] {
        if let Some(num) = s.strip_suffix(suffix) {
            if let Ok(n) = num.trim_start_matches('+').parse::<i64>() {
                let dt = now_local() + Duration::minutes(n * mins);
                return Ok(DateVal::DateTime(dt.with_second(0).and_then(|d| d.with_nanosecond(0)).unwrap_or(dt)));
            }
        }
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    // Trailing time ("9am", "14:30", "noon"), possibly split as "9 am".
    let (date_words, time) = split_time(&words);
    let date = if date_words.is_empty() {
        today
    } else {
        parse_date_words(&date_words.join(" "), today)
            .ok_or_else(|| crate::error::invalid(format!("can't read {input:?} as a date · try fri, +3d or 2026-10-09")))?
    };
    Ok(match time {
        Some(t) => DateVal::DateTime(date.and_time(t)),
        None => DateVal::Date(date),
    })
}

fn split_time<'a>(words: &[&'a str]) -> (Vec<&'a str>, Option<NaiveTime>) {
    let n = words.len();
    if n >= 2 && (words[n - 1] == "am" || words[n - 1] == "pm") {
        if let Some(t) = parse_time(&format!("{}{}", words[n - 2], words[n - 1])) {
            return (strip_at(&words[..n - 2]), Some(t));
        }
    }
    if n >= 1 {
        if let Some(t) = parse_time(words[n - 1]) {
            return (strip_at(&words[..n - 1]), Some(t));
        }
    }
    (words.to_vec(), None)
}

fn strip_at<'a>(words: &[&'a str]) -> Vec<&'a str> {
    let mut v = words.to_vec();
    if v.last() == Some(&"at") {
        v.pop();
    }
    v
}

pub fn parse_time(s: &str) -> Option<NaiveTime> {
    let s = s.trim().to_lowercase();
    match s.as_str() {
        "noon" => return NaiveTime::from_hms_opt(12, 0, 0),
        "midnight" => return NaiveTime::from_hms_opt(0, 0, 0),
        _ => {}
    }
    let (body, ampm) = if let Some(b) = s.strip_suffix("am") {
        (b, Some(false))
    } else if let Some(b) = s.strip_suffix("pm") {
        (b, Some(true))
    } else {
        (s.as_str(), None)
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None if ampm.is_some() => (body.parse::<u32>().ok()?, 0),
        None => return None, // bare numbers are not times
    };
    let h = match ampm {
        Some(pm) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            (h % 12) + if pm { 12 } else { 0 }
        }
        None => h,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

pub fn weekday(s: &str) -> Option<Weekday> {
    Some(match s {
        "mon" | "monday" | "mo" => Weekday::Mon,
        "tue" | "tues" | "tuesday" | "tu" => Weekday::Tue,
        "wed" | "weds" | "wednesday" | "we" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" | "th" => Weekday::Thu,
        "fri" | "friday" | "fr" => Weekday::Fri,
        "sat" | "saturday" | "sa" => Weekday::Sat,
        "sun" | "sunday" | "su" => Weekday::Sun,
        _ => return None,
    })
}

fn month(s: &str) -> Option<u32> {
    let m = match s.get(..3)? {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    Some(m)
}

/// Next date (including today) that falls on `wd`.
pub fn next_weekday(from: NaiveDate, wd: Weekday, strictly_after: bool) -> NaiveDate {
    let mut d = if strictly_after { from + Duration::days(1) } else { from };
    while d.weekday() != wd {
        d += Duration::days(1);
    }
    d
}

pub fn add_months(d: NaiveDate, months: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + months;
    let (y, m0) = (total.div_euclid(12), total.rem_euclid(12) as u32);
    let mut day = d.day();
    loop {
        if let Some(nd) = NaiveDate::from_ymd_opt(y, m0 + 1, day) {
            return nd;
        }
        day -= 1;
    }
}

fn end_of_month(d: NaiveDate) -> NaiveDate {
    add_months(NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap(), 1) - Duration::days(1)
}

/// Offsets like `+3d`, `-1w`, `2m`, `1y`, `+12h` (hours only meaningful for durations).
pub fn parse_offset_days(s: &str, today: NaiveDate) -> Option<NaiveDate> {
    let (sign, body) = match s.strip_prefix('-') {
        Some(b) => (-1i64, b),
        None => (1, s.strip_prefix('+').unwrap_or(s)),
    };
    let unit = body.chars().last()?;
    let n: i64 = body[..body.len() - unit.len_utf8()].parse().ok()?;
    let n = n * sign;
    Some(match unit {
        'd' => today + Duration::days(n),
        'w' => today + Duration::weeks(n),
        'm' => add_months(today, n as i32),
        'y' => add_months(today, (n * 12) as i32),
        _ => return None,
    })
}

/// A bare `m` in user input (`30m`, `+2m`) is ambiguous between minutes and months, so input
/// surfaces reject it: `"30m" is ambiguous here · use 30min or 30mo` (validation, exit 6).
/// Lenient surfaces (TUI, ThoughtBar, daemon `capture`) keep the token as text with a notice.
pub fn reject_bare_m(s: &str) -> Result<()> {
    for w in s.split_whitespace() {
        let body = w.trim_matches('"').trim_start_matches(['+', '-']);
        if body.len() > 1 && body.ends_with('m') && body[..body.len() - 1].chars().all(|c| c.is_ascii_digit()) {
            let n = &body[..body.len() - 1];
            return Err(crate::error::invalid(format!("\"{body}\" is ambiguous here · use {n}min or {n}mo")));
        }
    }
    Ok(())
}

/// A duration typed by a person (`--before 15min`, snooze `1h`): rejects a bare `m`, then
/// [`parse_duration`].
pub fn user_duration(s: &str) -> Result<Duration> {
    reject_bare_m(s)?;
    parse_duration(s)
}

/// The *stored* duration format, as in `alert.add` `trigger.offset` (docs/FORMAT.md): `-1d`,
/// `15m`, `2h`, `1w`, where `m` means **minutes**, forever. Replay re-reads stored offsets
/// with this, so it must never change meaning. User input goes through [`user_duration`].
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim().to_lowercase();
    let (sign, body) = match s.strip_prefix('-') {
        Some(b) => (-1i64, b.to_string()),
        None => (1, s.strip_prefix('+').unwrap_or(&s).to_string()),
    };
    let split = body.find(|c: char| !c.is_ascii_digit()).ok_or_else(|| crate::error::invalid(format!("bad duration {s:?} (use e.g. 15min, 2h, 1d, 1w)")))?;
    let (num, unit) = body.split_at(split);
    let n: i64 = num.parse().map_err(|_| crate::error::invalid(format!("bad duration {s:?} (use e.g. 15min, 2h, 1d, 1w)")))?;
    let d = match unit.trim() {
        "m" | "min" | "mins" => Duration::minutes(n),
        "h" | "hr" | "hrs" => Duration::hours(n),
        "d" => Duration::days(n),
        "w" => Duration::weeks(n),
        "mo" => Duration::days(30 * n),
        _ => return Err(crate::error::invalid(format!("bad duration unit in {s:?} (use min, h, d, w or mo)"))),
    };
    Ok(d * sign as i32)
}

fn parse_date_words(s: &str, today: NaiveDate) -> Option<NaiveDate> {
    let s = s.trim();
    match s {
        "today" | "tod" | "now" => return Some(today),
        "tomorrow" | "tom" | "tmr" | "tmrw" => return Some(today + Duration::days(1)),
        "yesterday" => return Some(today - Duration::days(1)),
        "next week" => return Some(next_weekday(today, Weekday::Mon, true)),
        "next month" => return Some(add_months(NaiveDate::from_ymd_opt(today.year(), today.month(), 1)?, 1)),
        "eow" | "end of week" => return Some(next_weekday(today, Weekday::Sun, false)),
        "eom" | "end of month" => return Some(end_of_month(today)),
        "eoy" | "end of year" => return NaiveDate::from_ymd_opt(today.year(), 12, 31),
        _ => {}
    }
    if let Some(wd) = weekday(s) {
        return Some(next_weekday(today, wd, false));
    }
    if let Some(rest) = s.strip_prefix("next ") {
        if let Some(wd) = weekday(rest) {
            return Some(next_weekday(today, wd, true));
        }
    }
    if let Some(rest) = s.strip_prefix("in ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() == 2 {
            let n: i64 = parts[0].parse().ok()?;
            let unit = parts[1].trim_end_matches('s');
            return Some(match unit {
                "day" => today + Duration::days(n),
                "week" => today + Duration::weeks(n),
                "month" => add_months(today, n as i32),
                "year" => add_months(today, (n * 12) as i32),
                _ => return None,
            });
        }
    }
    if let Some(d) = parse_offset_days(s, today) {
        return Some(d);
    }
    // MM-DD or MM/DD (upcoming)
    for sep in ['-', '/'] {
        let parts: Vec<&str> = s.split(sep).collect();
        if parts.len() == 2 {
            if let (Ok(m), Ok(d)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                return upcoming(today, m, d);
            }
        }
    }
    // "nov 1", "november 1st", "1 nov", "nov 1 2027"
    let parts: Vec<&str> = s.split(|c: char| c.is_whitespace() || c == ',').filter(|p| !p.is_empty()).collect();
    if parts.len() == 2 || parts.len() == 3 {
        let day_of = |p: &str| -> Option<u32> {
            p.trim_end_matches("st").trim_end_matches("nd").trim_end_matches("rd").trim_end_matches("th").parse().ok()
        };
        let (m, d) = if let Some(m) = month(parts[0]) {
            (m, day_of(parts[1])?)
        } else if let Some(m) = month(parts[1]) {
            (m, day_of(parts[0])?)
        } else {
            return None;
        };
        if parts.len() == 3 {
            let y: i32 = parts[2].parse().ok()?;
            return NaiveDate::from_ymd_opt(y, m, d);
        }
        return upcoming(today, m, d);
    }
    None
}

fn upcoming(today: NaiveDate, m: u32, d: u32) -> Option<NaiveDate> {
    let this_year = NaiveDate::from_ymd_opt(today.year(), m, d)?;
    if this_year >= today { Some(this_year) } else { NaiveDate::from_ymd_opt(today.year() + 1, m, d) }
}

/// Human-friendly relative description, e.g. "today", "in 3d", "2d ago".
pub fn relative(d: NaiveDate, today: NaiveDate) -> String {
    let days = (d - today).num_days();
    match days {
        0 => "today".into(),
        1 => "tomorrow".into(),
        -1 => "yesterday".into(),
        n if n > 0 && n < 7 => d.format("%a").to_string().to_lowercase(),
        n if n > 0 => format!("in {n}d"),
        n => format!("{}d ago", -n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    // 2026-10-03 is a Saturday.
    const SAT: (i32, u32, u32) = (2026, 10, 3);

    fn p(s: &str) -> String {
        parse(s, d(SAT.0, SAT.1, SAT.2)).unwrap().fmt()
    }

    #[test]
    fn relative_words() {
        assert_eq!(p("today"), "2026-10-03");
        assert_eq!(p("tomorrow"), "2026-10-04");
        assert_eq!(p("sat"), "2026-10-03");
        assert_eq!(p("next sat"), "2026-10-10");
        assert_eq!(p("fri"), "2026-10-09");
        assert_eq!(p("mon"), "2026-10-05");
        assert_eq!(p("+3d"), "2026-10-06");
        assert_eq!(p("-1w"), "2026-09-26");
        assert_eq!(p("in 2 weeks"), "2026-10-17");
        assert_eq!(p("next week"), "2026-10-05");
        assert_eq!(p("eom"), "2026-10-31");
    }

    #[test]
    fn absolute_and_times() {
        assert_eq!(p("2026-12-01"), "2026-12-01");
        assert_eq!(p("nov 1"), "2026-11-01");
        assert_eq!(p("1st nov"), "2026-11-01");
        assert_eq!(p("jan 5"), "2027-01-05");
        assert_eq!(p("nov 1 9am"), "2026-11-01T09:00");
        assert_eq!(p("tomorrow at 2:30pm"), "2026-10-04T14:30");
        assert_eq!(p("fri 14:00"), "2026-10-09T14:00");
        assert_eq!(p("9 am"), "2026-10-03T09:00");
        assert_eq!(p("2026-10-06T14:00"), "2026-10-06T14:00");
        assert!(parse("blorp", d(2026, 10, 3)).is_err());
    }

    #[test]
    fn month_math_clamps() {
        assert_eq!(add_months(d(2026, 1, 31), 1), d(2026, 2, 28));
        assert_eq!(add_months(d(2026, 12, 15), 1), d(2027, 1, 15));
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("-1d").unwrap(), Duration::days(-1));
        assert_eq!(parse_duration("15m").unwrap(), Duration::minutes(15));
        assert_eq!(parse_duration("30min").unwrap(), Duration::minutes(30));
        assert_eq!(parse_duration("2mo").unwrap(), Duration::days(60));
        assert!(parse_duration("3x").is_err());
        // User input: a bare m is ambiguous; the stored format keeps m = minutes.
        assert!(user_duration("15m").unwrap_err().to_string().contains("\"15m\" is ambiguous here · use 15min or 15mo"));
        assert_eq!(user_duration("15min").unwrap(), Duration::minutes(15));
        let t = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        assert!(parse("+2m", t).unwrap_err().to_string().contains("use 2min or 2mo"));
        assert_eq!(parse("+2mo", t).unwrap().date(), NaiveDate::from_ymd_opt(2026, 12, 3).unwrap());
    }
}
