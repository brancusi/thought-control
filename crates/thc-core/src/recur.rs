//! Recurrence: an RRULE subset plus Org-style completion modes.

use crate::dates::{add_months, next_weekday, weekday};
use anyhow::{Result, bail};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Next = previous date + interval (Org `+`). May remain overdue.
    Fixed,
    /// Next = previous date + interval, repeated until in the future (Org `++`).
    CatchUp,
    /// Next = completion date + interval (Org `.+`, Todoist `every!`).
    FromDone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repeat {
    /// RFC 5545 RRULE subset, e.g. `FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE`.
    pub rule: String,
    pub mode: Mode,
    /// What the user typed, for display.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    freq: Freq,
    interval: u32,
    byday: Vec<Weekday>,
    bymonthday: Option<u32>,
}

const WD_CODES: [(Weekday, &str); 7] = [
    (Weekday::Mon, "MO"),
    (Weekday::Tue, "TU"),
    (Weekday::Wed, "WE"),
    (Weekday::Thu, "TH"),
    (Weekday::Fri, "FR"),
    (Weekday::Sat, "SA"),
    (Weekday::Sun, "SU"),
];

impl Rule {
    fn to_rrule(&self) -> String {
        let freq = match self.freq {
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
            Freq::Yearly => "YEARLY",
        };
        let mut s = format!("FREQ={freq}");
        if self.interval != 1 {
            s.push_str(&format!(";INTERVAL={}", self.interval));
        }
        if !self.byday.is_empty() {
            let codes: Vec<&str> = self
                .byday
                .iter()
                .map(|w| WD_CODES.iter().find(|(d, _)| d == w).unwrap().1)
                .collect();
            s.push_str(&format!(";BYDAY={}", codes.join(",")));
        }
        if let Some(md) = self.bymonthday {
            s.push_str(&format!(";BYMONTHDAY={md}"));
        }
        s
    }

    fn from_rrule(s: &str) -> Result<Rule> {
        let mut r = Rule { freq: Freq::Daily, interval: 1, byday: vec![], bymonthday: None };
        for part in s.split(';') {
            let Some((k, v)) = part.split_once('=') else { continue };
            match k {
                "FREQ" => {
                    r.freq = match v {
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        _ => bail!("unsupported FREQ {v}"),
                    }
                }
                "INTERVAL" => r.interval = v.parse()?,
                "BYDAY" => {
                    r.byday = v
                        .split(',')
                        .filter_map(|c| WD_CODES.iter().find(|(_, code)| *code == c).map(|(d, _)| *d))
                        .collect()
                }
                "BYMONTHDAY" => r.bymonthday = Some(v.parse()?),
                _ => {}
            }
        }
        Ok(r)
    }

    /// First occurrence strictly after `d`.
    fn advance(&self, d: NaiveDate) -> NaiveDate {
        let n = self.interval.max(1) as i64;
        match self.freq {
            Freq::Daily => d + Duration::days(n),
            Freq::Weekly if self.byday.is_empty() => d + Duration::weeks(n),
            Freq::Weekly => {
                let mut c = d + Duration::days(1);
                loop {
                    // Crossing into a new week with INTERVAL>1 skips the gap weeks.
                    if c.weekday() == Weekday::Mon && n > 1 && c > d + Duration::days(1) {
                        c += Duration::weeks(n - 1);
                    }
                    if self.byday.contains(&c.weekday()) {
                        return c;
                    }
                    c += Duration::days(1);
                }
            }
            Freq::Monthly => {
                let base = add_months(d, n as i32);
                match self.bymonthday {
                    Some(md) => {
                        // Same month first if that day is still ahead and interval is 1.
                        if n == 1 {
                            if let Some(same) = clamp_day(d.year(), d.month(), md) {
                                if same > d {
                                    return same;
                                }
                            }
                        }
                        clamp_day(base.year(), base.month(), md).unwrap_or(base)
                    }
                    None => base,
                }
            }
            Freq::Yearly => add_months(d, (n * 12) as i32),
        }
    }
}

fn clamp_day(y: i32, m: u32, day: u32) -> Option<NaiveDate> {
    (1..=day).rev().find_map(|d| NaiveDate::from_ymd_opt(y, m, d))
}

impl Repeat {
    /// Parse "every 2w", "every weekday", "every mon,wed", "every month on the 1st",
    /// "daily", "every! week" (from completion). `mode` overrides when given.
    pub fn parse(input: &str, mode: Option<Mode>) -> Result<Repeat> {
        let raw = input.trim();
        let mut s = raw.to_lowercase();
        let mut parsed_mode = Mode::Fixed;
        if let Some(rest) = s.strip_prefix("every!") {
            parsed_mode = Mode::FromDone;
            s = format!("every {}", rest.trim());
        }
        for suffix in [" after done", " after completion", " from done", " from completion"] {
            if let Some(rest) = s.strip_suffix(suffix) {
                parsed_mode = Mode::FromDone;
                s = rest.to_string();
            }
        }
        if let Some(rest) = s.strip_suffix(" catch up") {
            parsed_mode = Mode::CatchUp;
            s = rest.to_string();
        }
        let body = s.strip_prefix("every").map(str::trim).unwrap_or(&s).to_string();
        // `every 1m` is ambiguous; a per-minute repeat isn't a thing, so the hint is months.
        // Stored rules are RRULEs, so this only affects new input.
        if let Some(n) = body.strip_suffix('m').filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            return Err(crate::error::invalid(format!("\"{body}\" is ambiguous here · use {n}mo for months")));
        }
        // Shorthand reads as words in the meta: `every! 1mo` → `every! month`, `every 2w` → `every 2 weeks`.
        let words = shorthand_words(&body);
        let body = match body.strip_suffix("mo") {
            Some(n) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => format!("{n}m"),
            _ => body,
        };
        let rule = parse_rule(&body)?;
        let text = match words {
            Some(w) => format!("{}{w}", if raw.to_lowercase().starts_with("every!") { "every! " } else { "every " }),
            None => raw.to_string(),
        };
        Ok(Repeat { rule: rule.to_rrule(), mode: mode.unwrap_or(parsed_mode), text })
    }

    /// Next date for the primary (scheduled or due) date after completion.
    pub fn next_date(&self, current: NaiveDate, done_on: NaiveDate, today: NaiveDate) -> Result<NaiveDate> {
        let rule = Rule::from_rrule(&self.rule)?;
        Ok(match self.mode {
            Mode::Fixed => rule.advance(current),
            Mode::CatchUp => {
                let mut d = rule.advance(current);
                while d <= today {
                    d = rule.advance(d);
                }
                d
            }
            Mode::FromDone => rule.advance(done_on),
        })
    }

    /// First occurrence on/after `from` (used when a repeat is set without a date).
    pub fn first_on_or_after(&self, from: NaiveDate) -> Result<NaiveDate> {
        let rule = Rule::from_rrule(&self.rule)?;
        let matches = match rule.freq {
            Freq::Weekly if !rule.byday.is_empty() => rule.byday.contains(&from.weekday()),
            Freq::Monthly => rule.bymonthday.is_none_or(|md| from.day() == md),
            _ => true,
        };
        if matches { Ok(from) } else { Ok(rule.advance(from)) }
    }
}

/// `3d` → `3 days`, `1w` → `week`, `2mo` → `2 months`, `1y` → `year`; None for anything else.
fn shorthand_words(body: &str) -> Option<String> {
    let b = body.trim();
    let (n, unit) = if let Some(n) = b.strip_suffix("mo") { (n, "month") } else {
        let u = b.chars().last()?;
        let unit = match u { 'd' => "day", 'w' => "week", 'y' => "year", _ => return None };
        (&b[..b.len() - 1], unit)
    };
    let n: u32 = n.parse().ok()?;
    Some(if n == 1 { unit.to_string() } else { format!("{n} {unit}s") })
}

fn parse_rule(body: &str) -> Result<Rule> {
    let b = body.trim();
    let simple = |freq, interval| Ok(Rule { freq, interval, byday: vec![], bymonthday: None });
    match b {
        "day" | "daily" | "1d" => return simple(Freq::Daily, 1),
        "week" | "weekly" | "1w" => return simple(Freq::Weekly, 1),
        "month" | "monthly" | "1m" => return simple(Freq::Monthly, 1),
        "year" | "yearly" | "annually" | "1y" => return simple(Freq::Yearly, 1),
        "weekday" | "weekdays" | "workday" => {
            return Ok(Rule {
                freq: Freq::Weekly,
                interval: 1,
                byday: vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri],
                bymonthday: None,
            });
        }
        "weekend" | "weekends" => {
            return Ok(Rule { freq: Freq::Weekly, interval: 1, byday: vec![Weekday::Sat, Weekday::Sun], bymonthday: None });
        }
        _ => {}
    }
    // "2w", "3d", "6m", "1y"
    if let Some(unit) = b.chars().last() {
        if let Ok(n) = b[..b.len() - unit.len_utf8()].parse::<u32>() {
            let freq = match unit {
                'd' => Some(Freq::Daily),
                'w' => Some(Freq::Weekly),
                'm' => Some(Freq::Monthly),
                'y' => Some(Freq::Yearly),
                _ => None,
            };
            if let Some(f) = freq {
                return simple(f, n);
            }
        }
    }
    // "2 weeks", "3 days"
    let words: Vec<&str> = b.split_whitespace().collect();
    if words.len() == 2 {
        if let Ok(n) = words[0].parse::<u32>() {
            let f = match words[1].trim_end_matches('s') {
                "day" => Some(Freq::Daily),
                "week" => Some(Freq::Weekly),
                "month" => Some(Freq::Monthly),
                "year" => Some(Freq::Yearly),
                _ => None,
            };
            if let Some(f) = f {
                return simple(f, n);
            }
        }
    }
    // "month on the 1st", "15th", "month on 15"
    let md_src = b.strip_prefix("month").map(str::trim).unwrap_or(b);
    let md_src = md_src.strip_prefix("on").map(str::trim).unwrap_or(md_src);
    let md_src = md_src.strip_prefix("the").map(str::trim).unwrap_or(md_src);
    let md_digits = md_src.trim_end_matches("st").trim_end_matches("nd").trim_end_matches("rd").trim_end_matches("th");
    if let Ok(md) = md_digits.parse::<u32>() {
        if (1..=31).contains(&md) {
            return Ok(Rule { freq: Freq::Monthly, interval: 1, byday: vec![], bymonthday: Some(md) });
        }
    }
    // "mon,wed", "monday and friday", "tue thu"
    let days: Vec<Weekday> = b
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|w| !w.is_empty() && *w != "and")
        .map(weekday)
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default();
    if !days.is_empty() {
        return Ok(Rule { freq: Freq::Weekly, interval: 1, byday: days, bymonthday: None });
    }
    bail!("could not understand repeat {body:?} (try: every day, every 2w, every weekday, every mon,thu, every month on the 1st)")
}

/// Shift a date to keep the same gap relative to a moved primary date.
pub fn shift(d: NaiveDate, old_primary: NaiveDate, new_primary: NaiveDate) -> NaiveDate {
    d + (new_primary - old_primary)
}

/// Helper used by tests and callers that need the next weekday occurrence.
pub fn upcoming_weekday(from: NaiveDate, wd: Weekday) -> NaiveDate {
    next_weekday(from, wd, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn parses_common_forms() {
        assert_eq!(Repeat::parse("every day", None).unwrap().rule, "FREQ=DAILY");
        assert_eq!(Repeat::parse("every 2w", None).unwrap().rule, "FREQ=WEEKLY;INTERVAL=2");
        assert_eq!(Repeat::parse("every weekday", None).unwrap().rule, "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR");
        assert_eq!(Repeat::parse("every mon,thu", None).unwrap().rule, "FREQ=WEEKLY;BYDAY=MO,TH");
        assert_eq!(Repeat::parse("every month on the 1st", None).unwrap().rule, "FREQ=MONTHLY;BYMONTHDAY=1");
        assert_eq!(Repeat::parse("every! week", None).unwrap().mode, Mode::FromDone);
        assert_eq!(Repeat::parse("every 3 days after done", None).unwrap().mode, Mode::FromDone);
        assert!(Repeat::parse("every blue moon", None).is_err());
    }

    #[test]
    fn next_dates_by_mode() {
        let wk = Repeat::parse("every week", None).unwrap();
        let today = d(2026, 10, 20);
        // Fixed: stays anchored even if overdue.
        assert_eq!(wk.next_date(d(2026, 10, 1), today, today).unwrap(), d(2026, 10, 8));
        // Catch-up: jumps past today.
        let cu = Repeat { mode: Mode::CatchUp, ..wk.clone() };
        assert_eq!(cu.next_date(d(2026, 10, 1), today, today).unwrap(), d(2026, 10, 22));
        // From done.
        let fd = Repeat { mode: Mode::FromDone, ..wk };
        assert_eq!(fd.next_date(d(2026, 10, 1), today, today).unwrap(), d(2026, 10, 27));
    }

    #[test]
    fn weekday_and_monthday_rules() {
        let wkd = Repeat::parse("every weekday", None).unwrap();
        // Fri 2026-10-09 -> Mon 2026-10-12
        assert_eq!(wkd.next_date(d(2026, 10, 9), d(2026, 10, 9), d(2026, 10, 9)).unwrap(), d(2026, 10, 12));
        let first = Repeat::parse("every month on the 31st", None).unwrap();
        assert_eq!(first.next_date(d(2026, 1, 31), d(2026, 1, 31), d(2026, 1, 31)).unwrap(), d(2026, 2, 28));
        assert_eq!(first.first_on_or_after(d(2026, 10, 3)).unwrap(), d(2026, 10, 31));
    }
}
