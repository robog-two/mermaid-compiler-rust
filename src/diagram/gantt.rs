//! `gantt` diagrams.
//!
//! Pipeline: [`parse`] (text → [`Gantt`], with all task dates resolved) →
//! [`draw`] (→ [`Scene`]). Geometry follows mermaid's `ganttRenderer.ts`
//! (barHeight 20, barGap 4, topPadding 50, side paddings 75, gridLineStartPadding 35).
//!
//! Dates are handled without any date crate or clock:
//! * every instant is an `i64` count of milliseconds since 1970-01-01T00:00 **UTC**
//!   (mermaid uses the browser's local zone; we pin UTC so output is reproducible);
//! * civil dates use Howard Hinnant's `days_from_civil` / `civil_from_days`;
//! * formats missing a date part (e.g. `dateFormat HH:mm`) default to 1970-01-01,
//!   where mermaid/dayjs would use *today*;
//! * `todayMarker` is parsed but never drawn: drawing it needs the wall clock, which
//!   would make the output non-deterministic.

use crate::color::{self, Rgba};
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{line_width, normalize_label};
use crate::theme::Palette;

pub const SECOND_MS: i64 = 1_000;
pub const MINUTE_MS: i64 = 60 * SECOND_MS;
pub const HOUR_MS: i64 = 60 * MINUTE_MS;
pub const DAY_MS: i64 = 24 * HOUR_MS;

// ---------------------------------------------------------------------------
// Civil date arithmetic (proleptic Gregorian, UTC)
// ---------------------------------------------------------------------------

/// Days since 1970-01-01 for a civil date (`m` 1..=12, `d` 1..=31).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let mp = (m + 9) % 12; // March = 0
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date `(year, month, day)` for a day count since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

pub fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(y) => 29,
        _ => 28,
    }
}

/// Day of week, 0 = Sunday.
pub fn weekday(ms: i64) -> u32 {
    (ms.div_euclid(DAY_MS) + 4).rem_euclid(7) as u32
}

/// Broken-down UTC time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub milli: u32,
}

impl DateTime {
    pub fn from_ms(ms: i64) -> Self {
        let days = ms.div_euclid(DAY_MS);
        let rem = ms.rem_euclid(DAY_MS);
        let (year, month, day) = civil_from_days(days);
        DateTime {
            year,
            month,
            day,
            hour: (rem / HOUR_MS) as u32,
            minute: (rem % HOUR_MS / MINUTE_MS) as u32,
            second: (rem % MINUTE_MS / SECOND_MS) as u32,
            milli: (rem % SECOND_MS) as u32,
        }
    }

    pub fn to_ms(self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * DAY_MS
            + self.hour as i64 * HOUR_MS
            + self.minute as i64 * MINUTE_MS
            + self.second as i64 * SECOND_MS
            + self.milli as i64
    }
}

/// Add calendar months, clamping the day (Jan 31 + 1M = Feb 28/29), like dayjs.
pub fn add_months(ms: i64, months: i64) -> i64 {
    let mut dt = DateTime::from_ms(ms);
    let total = dt.year * 12 + (dt.month as i64 - 1) + months;
    dt.year = total.div_euclid(12);
    dt.month = (total.rem_euclid(12) + 1) as u32;
    dt.day = dt.day.min(days_in_month(dt.year, dt.month));
    dt.to_ms()
}

/// A mermaid task duration (`3d`, `1.5h`, `2w`, `1M`, `250ms`, ...).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Duration {
    pub value: f64,
    pub unit: DurUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurUnit {
    Millisecond,
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

/// Parse mermaid's duration syntax `^(\d+(\.\d+)?)(ms|[Mdhmswy])$`.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (num, unit) = s.split_at(split);
    if num.is_empty() || num.starts_with('.') || num.ends_with('.') || num.matches('.').count() > 1
    {
        return None;
    }
    let value: f64 = num.parse().ok()?;
    let unit = match unit {
        "ms" => DurUnit::Millisecond,
        "s" => DurUnit::Second,
        "m" => DurUnit::Minute,
        "h" => DurUnit::Hour,
        "d" => DurUnit::Day,
        "w" => DurUnit::Week,
        "M" => DurUnit::Month,
        "y" => DurUnit::Year,
        _ => return None,
    };
    Some(Duration { value, unit })
}

impl Duration {
    /// `start + self`. Whole months/years are calendar-aware; fractions are
    /// approximated with 30-day months and 365-day years.
    pub fn add_to(self, start: i64) -> i64 {
        let fixed = |unit_ms: i64| start + (self.value * unit_ms as f64).round() as i64;
        match self.unit {
            DurUnit::Millisecond => fixed(1),
            DurUnit::Second => fixed(SECOND_MS),
            DurUnit::Minute => fixed(MINUTE_MS),
            DurUnit::Hour => fixed(HOUR_MS),
            DurUnit::Day => fixed(DAY_MS),
            DurUnit::Week => fixed(7 * DAY_MS),
            DurUnit::Month | DurUnit::Year => {
                let per = if self.unit == DurUnit::Year { 12 } else { 1 };
                let whole = self.value.trunc();
                let after = add_months(start, whole as i64 * per);
                let frac_days = (self.value - whole) * if per == 12 { 365.0 } else { 30.0 };
                after + (frac_days * DAY_MS as f64).round() as i64
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Input date formats (dayjs / moment tokens)
// ---------------------------------------------------------------------------

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Lit(String),
    Year4,
    Year2,
    Month(usize),
    MonthShort,
    MonthLong,
    Day(usize),
    DayOrdinal,
    WeekdayName,
    Hour24(usize),
    Hour12(usize),
    Minute(usize),
    Second(usize),
    Frac(usize),
    AmPm,
    UnixSec,
    UnixMs,
    Offset,
}

fn tokenize_format(fmt: &str) -> Vec<Tok> {
    const TABLE: [&str; 28] = [
        "YYYY", "MMMM", "dddd", "SSS", "MMM", "ddd", "YY", "MM", "Do", "DD", "HH", "hh", "mm",
        "ss", "SS", "ZZ", "M", "D", "H", "h", "m", "s", "S", "A", "a", "X", "x", "Z",
    ];
    let mut out = Vec::new();
    let mut rest = fmt;
    while let Some(c) = rest.chars().next() {
        if c == '[' {
            if let Some(end) = rest.find(']') {
                out.push(Tok::Lit(rest[1..end].to_string()));
                rest = &rest[end + 1..];
                continue;
            }
        }
        if let Some(t) = TABLE.iter().find(|t| rest.starts_with(**t)) {
            out.push(match *t {
                "YYYY" => Tok::Year4,
                "YY" => Tok::Year2,
                "MMMM" => Tok::MonthLong,
                "MMM" => Tok::MonthShort,
                "MM" => Tok::Month(2),
                "M" => Tok::Month(1),
                "Do" => Tok::DayOrdinal,
                "DD" => Tok::Day(2),
                "D" => Tok::Day(1),
                "dddd" | "ddd" => Tok::WeekdayName,
                "HH" => Tok::Hour24(2),
                "H" => Tok::Hour24(1),
                "hh" => Tok::Hour12(2),
                "h" => Tok::Hour12(1),
                "mm" => Tok::Minute(2),
                "m" => Tok::Minute(1),
                "ss" => Tok::Second(2),
                "s" => Tok::Second(1),
                "SSS" => Tok::Frac(3),
                "SS" => Tok::Frac(2),
                "S" => Tok::Frac(1),
                "A" | "a" => Tok::AmPm,
                "X" => Tok::UnixSec,
                "x" => Tok::UnixMs,
                _ => Tok::Offset,
            });
            rest = &rest[t.len()..];
            continue;
        }
        match out.last_mut() {
            Some(Tok::Lit(s)) => s.push(c),
            _ => out.push(Tok::Lit(c.to_string())),
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Read `min..=max` ASCII digits.
fn take_digits(s: &str, min: usize, max: usize) -> Option<(u32, &str)> {
    let n = s.bytes().take(max).take_while(u8::is_ascii_digit).count();
    if n < min {
        return None;
    }
    Some((s[..n].parse().ok()?, &s[n..]))
}

fn take_name<'a>(s: &'a str, names: &[&str], short: bool) -> Option<(usize, &'a str)> {
    names.iter().enumerate().find_map(|(i, n)| {
        let n = if short { &n[..3] } else { n };
        let head = s.get(..n.len())?;
        head.eq_ignore_ascii_case(n).then(|| (i, &s[n.len()..]))
    })
}

fn take_signed(s: &str) -> Option<(f64, &str)> {
    let n = s
        .char_indices()
        .take_while(|&(i, c)| c.is_ascii_digit() || c == '.' || (i == 0 && c == '-'))
        .count();
    Some((s[..n].parse().ok()?, &s[n..]))
}

/// Parse `input` strictly against a dayjs-style `format`. Returns UTC milliseconds.
pub fn parse_date(input: &str, format: &str) -> Option<i64> {
    let toks = tokenize_format(format.trim());
    let mut s = input.trim();
    let (mut y, mut mo, mut d) = (1970i64, 1u32, 1u32);
    let (mut h, mut mi, mut sec, mut ms) = (0u32, 0u32, 0u32, 0u32);
    let mut pm: Option<bool> = None;
    let mut twelve = false;
    let mut offset_min: i64 = 0;
    let mut absolute: Option<i64> = None;
    for t in &toks {
        match t {
            Tok::Lit(l) => s = s.strip_prefix(l.as_str())?,
            Tok::Year4 => {
                let (v, r) = take_digits(s, 4, 4)?;
                y = v as i64;
                s = r;
            }
            Tok::Year2 => {
                let (v, r) = take_digits(s, 2, 2)?;
                y = if v < 69 {
                    2000 + v as i64
                } else {
                    1900 + v as i64
                };
                s = r;
            }
            Tok::Month(w) => {
                let (v, r) = take_digits(s, *w, 2)?;
                mo = v;
                s = r;
            }
            Tok::MonthShort | Tok::MonthLong => {
                let (i, r) = take_name(s, &MONTHS, *t == Tok::MonthShort)?;
                mo = i as u32 + 1;
                s = r;
            }
            Tok::Day(w) => {
                let (v, r) = take_digits(s, *w, 2)?;
                d = v;
                s = r;
            }
            Tok::DayOrdinal => {
                let (v, r) = take_digits(s, 1, 2)?;
                d = v;
                s = ["st", "nd", "rd", "th"]
                    .iter()
                    .find_map(|suf| r.strip_prefix(suf))?;
            }
            Tok::WeekdayName => {
                let r = take_name(s, &WEEKDAYS, false)
                    .or_else(|| take_name(s, &WEEKDAYS, true))?
                    .1;
                s = r;
            }
            Tok::Hour24(w) | Tok::Hour12(w) => {
                let (v, r) = take_digits(s, *w, 2)?;
                h = v;
                twelve |= matches!(t, Tok::Hour12(_));
                s = r;
            }
            Tok::Minute(w) => {
                let (v, r) = take_digits(s, *w, 2)?;
                mi = v;
                s = r;
            }
            Tok::Second(w) => {
                let (v, r) = take_digits(s, *w, 2)?;
                sec = v;
                s = r;
            }
            Tok::Frac(n) => {
                let (v, r) = take_digits(s, *n, *n)?;
                ms = v * 10u32.pow(3 - (*n as u32).min(3));
                s = r;
            }
            Tok::AmPm => {
                let head = s.get(..2)?;
                pm = Some(match head.to_ascii_lowercase().as_str() {
                    "am" => false,
                    "pm" => true,
                    _ => return None,
                });
                s = &s[2..];
            }
            Tok::UnixSec => {
                let (v, r) = take_signed(s)?;
                absolute = Some((v * 1000.0).round() as i64);
                s = r;
            }
            Tok::UnixMs => {
                let (v, r) = take_signed(s)?;
                absolute = Some(v.round() as i64);
                s = r;
            }
            Tok::Offset => {
                if let Some(r) = s.strip_prefix('Z') {
                    s = r;
                    continue;
                }
                let sign = match s.chars().next()? {
                    '+' => 1,
                    '-' => -1,
                    _ => return None,
                };
                let (hh, r) = take_digits(&s[1..], 2, 2)?;
                let r = r.strip_prefix(':').unwrap_or(r);
                let (mm, r) = take_digits(r, 2, 2)?;
                offset_min = sign * (hh as i64 * 60 + mm as i64);
                s = r;
            }
        }
    }
    if !s.is_empty() {
        return None;
    }
    if let Some(a) = absolute {
        return Some(a);
    }
    if let Some(is_pm) = pm {
        if !(1..=12).contains(&h) && twelve {
            return None;
        }
        if is_pm && h < 12 {
            h += 12;
        } else if !is_pm && h == 12 {
            h = 0;
        }
    }
    if !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) || h > 23 || mi > 59 || sec > 59
    {
        return None;
    }
    let dt = DateTime {
        year: y,
        month: mo,
        day: d,
        hour: h,
        minute: mi,
        second: sec,
        milli: ms,
    };
    Some(dt.to_ms() - offset_min * MINUTE_MS)
}

/// mermaid falls back to `new Date(str)`; we accept the ISO-8601 shapes it handles.
fn parse_iso(input: &str) -> Option<i64> {
    [
        "YYYY-MM-DD",
        "YYYY-MM-DDTHH:mm",
        "YYYY-MM-DDTHH:mm:ss",
        "YYYY-MM-DDTHH:mm:ss.SSS",
        "YYYY-MM-DDTHH:mm:ssZ",
        "YYYY-MM-DDTHH:mm:ss.SSSZ",
        "YYYY-MM-DD HH:mm",
        "YYYY-MM-DD HH:mm:ss",
    ]
    .iter()
    .find_map(|f| parse_date(input, f))
}

// ---------------------------------------------------------------------------
// Output formats (d3 strftime subset)
// ---------------------------------------------------------------------------

/// Format `ms` with a d3-style `%` format (`%Y-%m-%d`, `%b %e`, `%H:%M`, ...).
pub fn format_time(ms: i64, fmt: &str) -> String {
    let dt = DateTime::from_ms(ms);
    let mut out = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut pad: Option<char> = None;
        if let Some(&p) = chars.peek() {
            if p == '-' || p == '_' || p == '0' {
                pad = Some(p);
                chars.next();
            }
        }
        let Some(spec) = chars.next() else {
            out.push('%');
            break;
        };
        let num = |v: i64, width: usize, default_pad: char| -> String {
            let s = v.to_string();
            match pad.unwrap_or(default_pad) {
                '-' => s,
                p => {
                    let fill = if p == '_' { ' ' } else { p };
                    let mut r: String =
                        std::iter::repeat_n(fill, width.saturating_sub(s.len())).collect();
                    r.push_str(&s);
                    r
                }
            }
        };
        let doy = days_from_civil(dt.year, dt.month, dt.day) - days_from_civil(dt.year, 1, 1);
        let wd = weekday(ms) as i64;
        let h12 = if dt.hour % 12 == 0 { 12 } else { dt.hour % 12 } as i64;
        let piece = match spec {
            'Y' => dt.year.to_string(),
            'y' => num(dt.year.rem_euclid(100), 2, '0'),
            'm' => num(dt.month as i64, 2, '0'),
            'd' => num(dt.day as i64, 2, '0'),
            'e' => num(dt.day as i64, 2, ' '),
            'b' | 'h' => MONTHS[dt.month as usize - 1][..3].to_string(),
            'B' => MONTHS[dt.month as usize - 1].to_string(),
            'a' => WEEKDAYS[wd as usize][..3].to_string(),
            'A' => WEEKDAYS[wd as usize].to_string(),
            'H' => num(dt.hour as i64, 2, '0'),
            'I' => num(h12, 2, '0'),
            'M' => num(dt.minute as i64, 2, '0'),
            'S' => num(dt.second as i64, 2, '0'),
            'L' => num(dt.milli as i64, 3, '0'),
            'p' => if dt.hour < 12 { "AM" } else { "PM" }.to_string(),
            'j' => num(doy + 1, 3, '0'),
            'w' => wd.to_string(),
            'u' => (if wd == 0 { 7 } else { wd }).to_string(),
            'U' => {
                let jan1 = weekday(days_from_civil(dt.year, 1, 1) * DAY_MS) as i64;
                num((doy + jan1) / 7, 2, '0')
            }
            'W' => {
                let jan1 = (weekday(days_from_civil(dt.year, 1, 1) * DAY_MS) as i64 + 6) % 7;
                num((doy + jan1) / 7, 2, '0')
            }
            'Q' => ms.to_string(),
            's' => ms.div_euclid(1000).to_string(),
            'x' => format_time(ms, "%-m/%-d/%Y"),
            'X' => format_time(ms, "%-I:%M:%S %p"),
            'c' => format_time(ms, "%-m/%-d/%Y, %-I:%M:%S %p"),
            'Z' => "+0000".to_string(),
            '%' => "%".to_string(),
            other => format!("%{other}"),
        };
        out.push_str(&piece);
    }
    out
}

// ---------------------------------------------------------------------------
// Axis ticks (d3 time-scale tick selection)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickUnit {
    Millisecond,
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl TickUnit {
    fn approx_ms(self) -> i64 {
        match self {
            TickUnit::Millisecond => 1,
            TickUnit::Second => SECOND_MS,
            TickUnit::Minute => MINUTE_MS,
            TickUnit::Hour => HOUR_MS,
            TickUnit::Day => DAY_MS,
            TickUnit::Week => 7 * DAY_MS,
            TickUnit::Month => 30 * DAY_MS,
            TickUnit::Year => 365 * DAY_MS,
        }
    }
}

/// Parse mermaid's `tickInterval` (`1day`, `2week`, `6hour`, ...).
pub fn parse_tick_interval(s: &str) -> Option<(i64, TickUnit)> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    let n: i64 = s[..split].parse().ok().filter(|n| *n > 0)?;
    let unit = match &s[split..] {
        "millisecond" => TickUnit::Millisecond,
        "second" => TickUnit::Second,
        "minute" => TickUnit::Minute,
        "hour" => TickUnit::Hour,
        "day" => TickUnit::Day,
        "week" => TickUnit::Week,
        "month" => TickUnit::Month,
        _ => return None,
    };
    Some((n, unit))
}

/// d3's `tickStep`: a 1/2/5×10ᵏ step giving about `count` ticks.
fn nice_step(span: f64, count: f64) -> f64 {
    let raw = (span / count).max(f64::MIN_POSITIVE);
    let power = 10f64.powf(raw.log10().floor());
    let err = raw / power;
    let f = if err >= 50f64.sqrt() {
        10.0
    } else if err >= 10f64.sqrt() {
        5.0
    } else if err >= 2f64.sqrt() {
        2.0
    } else {
        1.0
    };
    (f * power).max(1.0)
}

/// Choose the tick interval d3 would for ~`count` ticks on `[t0, t1]`.
pub fn auto_interval(t0: i64, t1: i64, count: f64) -> (i64, TickUnit) {
    use TickUnit::*;
    const IV: [(TickUnit, i64); 18] = [
        (Second, 1),
        (Second, 5),
        (Second, 15),
        (Second, 30),
        (Minute, 1),
        (Minute, 5),
        (Minute, 15),
        (Minute, 30),
        (Hour, 1),
        (Hour, 3),
        (Hour, 6),
        (Hour, 12),
        (Day, 1),
        (Day, 2),
        (Week, 1),
        (Month, 1),
        (Month, 3),
        (Year, 1),
    ];
    let dur = |(u, n): (TickUnit, i64)| (u.approx_ms() * n) as f64;
    let span = (t1 - t0).max(1) as f64;
    let target = span / count;
    let i = IV.iter().position(|&iv| dur(iv) > target);
    match i {
        None => {
            let years = nice_step(span / Year.approx_ms() as f64, count);
            (years as i64, Year)
        }
        Some(0) => (nice_step(span, count) as i64, Millisecond),
        Some(i) => {
            if target / dur(IV[i - 1]) < dur(IV[i]) / target {
                (IV[i - 1].1, IV[i - 1].0)
            } else {
                (IV[i].1, IV[i].0)
            }
        }
    }
}

/// Tick instants in `[t0, t1]` for `step × unit`. Weeks start on `week_start` (0 = Sunday).
pub fn ticks(t0: i64, t1: i64, step: i64, unit: TickUnit, week_start: u32) -> Vec<i64> {
    const MAX: usize = 2000;
    let step = step.max(1);
    let mut out = Vec::new();
    match unit {
        TickUnit::Millisecond | TickUnit::Second | TickUnit::Minute | TickUnit::Hour => {
            let k = unit.approx_ms() * step;
            let mut t = t0.div_euclid(k) * k;
            if t < t0 {
                t += k;
            }
            while t <= t1 && out.len() < MAX {
                out.push(t);
                t += k;
            }
        }
        TickUnit::Day | TickUnit::Week => {
            let mut day = t0.div_euclid(DAY_MS);
            if day * DAY_MS < t0 {
                day += 1;
            }
            if unit == TickUnit::Week {
                while weekday(day * DAY_MS) != week_start {
                    day += 1;
                }
            }
            while day * DAY_MS <= t1 && out.len() < MAX {
                let t = day * DAY_MS;
                if unit == TickUnit::Week {
                    out.push(t);
                    day += 7 * step;
                } else {
                    let (_, _, dom) = civil_from_days(day);
                    if (dom as i64 - 1) % step == 0 {
                        out.push(t);
                    }
                    day += 1;
                }
            }
        }
        TickUnit::Month | TickUnit::Year => {
            let dt = DateTime::from_ms(t0);
            let (mut y, mut m) = (dt.year, dt.month);
            loop {
                let t = days_from_civil(y, m, 1) * DAY_MS;
                if t > t1 || out.len() >= MAX {
                    break;
                }
                let keep = match unit {
                    TickUnit::Month => (m as i64 - 1) % step == 0,
                    _ => m == 1 && y.rem_euclid(step) == 0,
                };
                if t >= t0 && keep {
                    out.push(t);
                }
                m += 1;
                if m > 12 {
                    m = 1;
                    y += 1;
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// IR + parser
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: String,
    pub label: String,
    pub section: usize,
    pub start: i64,
    /// Logical end (what `after` refers to).
    pub end: i64,
    /// Drawn end (excludes trailing excluded days, like mermaid's `renderEndTime`).
    pub render_end: i64,
    pub done: bool,
    pub active: bool,
    pub crit: bool,
    pub milestone: bool,
    pub vert: bool,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gantt {
    pub title: Option<String>,
    pub date_format: String,
    pub axis_format: Option<String>,
    pub tick_interval: Option<(i64, TickUnit)>,
    pub excludes: Vec<String>,
    pub includes: Vec<String>,
    /// First weekend day: 6 = Saturday (default) or 5 = Friday.
    pub weekend_start: u32,
    /// First day of week for week ticks (0 = Sunday).
    pub week_start: u32,
    pub inclusive_end_dates: bool,
    pub top_axis: bool,
    pub today_marker: Option<String>,
    pub sections: Vec<String>,
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq)]
enum StartSpec {
    Prev,
    At(i64),
    After(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
enum EndSpec {
    At(i64),
    For(Duration),
    Until(Vec<String>),
}

#[derive(Debug, Clone)]
struct RawTask {
    line: usize,
    label: String,
    id: String,
    section: usize,
    start: StartSpec,
    end: EndSpec,
    tags: [bool; 5], // active, done, crit, milestone, vert
}

fn weekday_index(name: &str) -> Option<u32> {
    WEEKDAYS
        .iter()
        .position(|w| w.eq_ignore_ascii_case(name))
        .map(|i| i as u32)
}

fn keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    if rest.is_empty() {
        return Some("");
    }
    rest.starts_with(char::is_whitespace)
        .then(|| rest.trim())
        .or_else(|| rest.strip_prefix(':').map(str::trim))
}

/// Parse a gantt body and resolve every task's dates.
pub fn parse(body: &str, title: Option<String>) -> Result<Gantt> {
    let mut g = Gantt {
        title,
        date_format: "YYYY-MM-DD".into(),
        axis_format: None,
        tick_interval: None,
        excludes: Vec::new(),
        includes: Vec::new(),
        weekend_start: 6,
        week_start: 0,
        inclusive_end_dates: false,
        top_axis: false,
        today_marker: None,
        sections: Vec::new(),
        tasks: Vec::new(),
    };
    let mut header = false;
    // Raw lines are kept: date parsing depends on the final `dateFormat`.
    let mut raw: Vec<(usize, usize, String, String)> = Vec::new(); // line, section, label, data
    for (i, l) in body.lines().enumerate() {
        let line_no = i + 1;
        let line = l.trim();
        if line.is_empty() {
            continue;
        }
        if !header {
            let rest = keyword(line, "gantt")
                .ok_or_else(|| Error::parse(line_no, format!("{line}\nExpecting 'gantt'")))?;
            header = true;
            if !rest.is_empty() {
                return Err(Error::parse(
                    line_no,
                    format!("{line}\nUnexpected text after 'gantt'"),
                ));
            }
            continue;
        }
        if let Some(v) = keyword(line, "dateFormat") {
            g.date_format = v.to_string();
        } else if let Some(v) = keyword(line, "axisFormat") {
            g.axis_format = Some(v.to_string());
        } else if let Some(v) = keyword(line, "tickInterval") {
            g.tick_interval = Some(parse_tick_interval(v).ok_or_else(|| {
                Error::parse(
                    line_no,
                    format!("{line}\nInvalid tickInterval (expected e.g. 1day, 2week, 6hour)"),
                )
            })?);
        } else if let Some(v) = keyword(line, "excludes") {
            g.excludes.extend(
                v.split([',', ' ', '\t'])
                    .filter(|s| !s.is_empty())
                    .map(str::to_lowercase),
            );
        } else if let Some(v) = keyword(line, "includes") {
            g.includes.extend(
                v.split([',', ' ', '\t'])
                    .filter(|s| !s.is_empty())
                    .map(str::to_lowercase),
            );
        } else if let Some(v) = keyword(line, "todayMarker") {
            g.today_marker = Some(v.to_string());
        } else if let Some(v) = keyword(line, "weekday") {
            g.week_start = weekday_index(v).ok_or_else(|| {
                Error::parse(line_no, format!("{line}\nExpecting a weekday name"))
            })?;
        } else if let Some(v) = keyword(line, "weekend") {
            g.weekend_start = match v.to_lowercase().as_str() {
                "friday" => 5,
                "saturday" => 6,
                _ => {
                    return Err(Error::parse(
                        line_no,
                        format!("{line}\nExpecting 'friday' or 'saturday'"),
                    ));
                }
            };
        } else if line == "inclusiveEndDates" {
            g.inclusive_end_dates = true;
        } else if line == "topAxis" {
            g.top_axis = true;
        } else if let Some(v) = keyword(line, "title") {
            g.title = Some(v.to_string());
        } else if let Some(v) = keyword(line, "section") {
            g.sections.push(v.to_string());
        } else if keyword(line, "click").is_some() {
            // Interactivity (href/call) has no meaning in a static image.
        } else if let Some(colon) = line.find(':') {
            let label = line[..colon].trim().to_string();
            if label.is_empty() {
                return Err(Error::parse(
                    line_no,
                    format!("{line}\nExpecting a task name before ':'"),
                ));
            }
            if g.sections.is_empty() {
                g.sections.push(String::new());
            }
            raw.push((
                line_no,
                g.sections.len() - 1,
                label,
                line[colon + 1..].to_string(),
            ));
        } else {
            return Err(Error::parse(
                line_no,
                format!("{line}\nExpecting a task ('name : metadata') or a gantt keyword"),
            ));
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'gantt'"));
    }
    let raws = raw
        .iter()
        .enumerate()
        .map(|(n, (line, sec, label, data))| parse_task(&g, *line, *sec, label, data, n + 1))
        .collect::<Result<Vec<_>>>()?;
    g.tasks = resolve(&g, raws)?;
    Ok(g)
}

fn parse_start(g: &Gantt, s: &str) -> Option<StartSpec> {
    if let Some(ids) = s.strip_prefix("after ") {
        let ids: Vec<String> = ids.split_whitespace().map(str::to_string).collect();
        return (!ids.is_empty()).then_some(StartSpec::After(ids));
    }
    parse_date(s, &g.date_format)
        .or_else(|| parse_iso(s))
        .map(StartSpec::At)
}

fn parse_end(g: &Gantt, s: &str) -> Option<EndSpec> {
    if let Some(ids) = s.strip_prefix("until ") {
        let ids: Vec<String> = ids.split_whitespace().map(str::to_string).collect();
        return (!ids.is_empty()).then_some(EndSpec::Until(ids));
    }
    if let Some(t) = parse_date(s, &g.date_format) {
        let t = if g.inclusive_end_dates { t + DAY_MS } else { t };
        return Some(EndSpec::At(t));
    }
    parse_duration(s).map(EndSpec::For)
}

fn parse_task(
    g: &Gantt,
    line: usize,
    section: usize,
    label: &str,
    data: &str,
    n: usize,
) -> Result<RawTask> {
    let mut parts: Vec<&str> = data.split(',').map(str::trim).collect();
    let mut tags = [false; 5];
    const TAGS: [&str; 5] = ["active", "done", "crit", "milestone", "vert"];
    while let Some(i) = parts.first().and_then(|p| TAGS.iter().position(|t| t == p)) {
        tags[i] = true;
        parts.remove(0);
    }
    let err = |msg: &str| Error::parse(line, format!("{label} :{data}\n{msg}"));
    if parts.iter().any(|p| p.is_empty()) || parts.is_empty() {
        return Err(err("Expecting task metadata: [tags,] [id,] [start,] end"));
    }
    let (id, start, end) = match parts.as_slice() {
        [end] => (None, None, *end),
        [start, end] => (None, Some(*start), *end),
        [id, start, end] => (Some(*id), Some(*start), *end),
        _ => return Err(err("Too many metadata items (at most: id, start, end)")),
    };
    let start = match start {
        None => StartSpec::Prev,
        Some(s) => parse_start(g, s).ok_or_else(|| {
            err(&format!(
                "Invalid start '{s}': expected a date in format '{}' or 'after <id>'",
                g.date_format
            ))
        })?,
    };
    let end = parse_end(g, end).ok_or_else(|| {
        err(&format!(
            "Invalid end '{end}': expected a date in format '{}', a duration (e.g. 3d) or 'until <id>'",
            g.date_format
        ))
    })?;
    Ok(RawTask {
        line,
        label: label.to_string(),
        id: id.map_or_else(|| format!("task{n}"), str::to_string),
        section,
        start,
        end,
        tags,
    })
}

fn is_excluded(g: &Gantt, t: i64) -> bool {
    if g.excludes.is_empty() {
        return false;
    }
    let day = t.div_euclid(DAY_MS);
    let on_day = |list: &[String]| {
        list.iter().any(|e| {
            parse_date(e, &g.date_format)
                .or_else(|| parse_iso(e))
                .is_some_and(|d| d.div_euclid(DAY_MS) == day)
        })
    };
    if on_day(&g.includes) {
        return false;
    }
    let wd = weekday(t);
    let weekend = wd == g.weekend_start || wd == (g.weekend_start + 1) % 7;
    if weekend && g.excludes.iter().any(|e| e == "weekends") {
        return true;
    }
    let name = WEEKDAYS[wd as usize].to_lowercase();
    g.excludes.contains(&name) || on_day(&g.excludes)
}

/// mermaid's `fixTaskDates`: push `end` past excluded days; returns `(end, render_end)`.
fn fix_task_dates(g: &Gantt, mut start: i64, mut end: i64) -> (i64, i64) {
    let mut invalid = false;
    let mut render_end = end;
    let mut guard = 0;
    while start <= end && guard < 100_000 {
        if !invalid {
            render_end = end;
        }
        invalid = is_excluded(g, start);
        if invalid {
            end += DAY_MS;
        }
        start += DAY_MS;
        guard += 1;
    }
    (end, render_end)
}

fn resolve(g: &Gantt, raws: Vec<RawTask>) -> Result<Vec<Task>> {
    let n = raws.len();
    let mut done: Vec<Option<Task>> = vec![None; n];
    let find = |done: &[Option<Task>], id: &str| -> Option<Option<Task>> {
        let idx = raws.iter().position(|r| r.id == id)?;
        Some(done[idx].clone())
    };
    loop {
        let mut progress = false;
        for i in 0..n {
            if done[i].is_some() {
                continue;
            }
            let r = &raws[i];
            let start = match &r.start {
                StartSpec::At(t) => Some(*t),
                StartSpec::Prev => {
                    if i == 0 {
                        return Err(Error::parse(
                            r.line,
                            format!(
                                "{}\nThe first task needs a start date (e.g. '2024-01-01, 3d')",
                                r.label
                            ),
                        ));
                    }
                    done[i - 1].as_ref().map(|p| p.end)
                }
                StartSpec::After(ids) => {
                    let mut latest: Option<i64> = None;
                    let mut ready = true;
                    for id in ids {
                        match find(&done, id) {
                            None => {
                                return Err(Error::parse(
                                    r.line,
                                    format!("{}\nUnknown task id '{id}' in 'after'", r.label),
                                ));
                            }
                            Some(None) => ready = false,
                            Some(Some(t)) => latest = Some(latest.map_or(t.end, |l| l.max(t.end))),
                        }
                    }
                    if ready { latest } else { None }
                }
            };
            let Some(start) = start else { continue };
            let (end, manual) = match &r.end {
                EndSpec::At(t) => (Some(*t), true),
                EndSpec::For(d) => (Some(d.add_to(start)), false),
                EndSpec::Until(ids) => {
                    let mut earliest: Option<i64> = None;
                    let mut ready = true;
                    for id in ids {
                        match find(&done, id) {
                            None => {
                                return Err(Error::parse(
                                    r.line,
                                    format!("{}\nUnknown task id '{id}' in 'until'", r.label),
                                ));
                            }
                            Some(None) => ready = false,
                            Some(Some(t)) => {
                                earliest = Some(earliest.map_or(t.start, |e| e.min(t.start)))
                            }
                        }
                    }
                    (if ready { earliest } else { None }, true)
                }
            };
            let Some(end) = end else { continue };
            let (end, render_end) = if manual {
                (end, end)
            } else {
                fix_task_dates(g, start, end)
            };
            done[i] = Some(Task {
                id: r.id.clone(),
                label: r.label.clone(),
                section: r.section,
                start,
                end,
                render_end,
                active: r.tags[0],
                done: r.tags[1],
                crit: r.tags[2],
                milestone: r.tags[3],
                vert: r.tags[4],
                line: r.line,
            });
            progress = true;
        }
        if done.iter().all(Option::is_some) {
            return Ok(done.into_iter().flatten().collect());
        }
        if !progress {
            let i = done.iter().position(Option::is_none).unwrap_or(0);
            return Err(Error::parse(
                raws[i].line,
                format!(
                    "{}\nCircular task dependency ('after'/'until' references form a cycle)",
                    raws[i].label
                ),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

struct Colors {
    task: Color,
    task_border: Color,
    active: Color,
    active_border: Color,
    done: Color,
    done_border: Color,
    crit: Color,
    crit_border: Color,
    sections: [(Color, f64); 4],
    grid: Color,
    exclude: Color,
    vert: Color,
    text: Color,
    title: Color,
}

impl Colors {
    fn new(pal: &Palette) -> Self {
        let base = color::parse(pal.main_bkg.css()).unwrap_or(Rgba {
            r: 0xec,
            g: 0xec,
            b: 0xff,
            a: 1.0,
        });
        let (h, s, _) = base.to_hsl();
        let s = if s < 5.0 { 0.0 } else { s.max(40.0) };
        let hsl =
            |dh: f64, sat: f64, l: f64| Color::new(Rgba::from_hsl(h + dh, sat, l, 1.0).to_hex());
        let dark = pal.dark;
        let pick = |light: &str, dark_c: &str| Color::new(if dark { dark_c } else { light });
        Colors {
            task: hsl(-5.0, s * 0.56, if dark { 42.0 } else { 70.0 }),
            task_border: hsl(2.0, s * 0.45, if dark { 65.0 } else { 52.0 }),
            active: hsl(-7.0, s, if dark { 28.0 } else { 87.0 }),
            active_border: hsl(2.0, s * 0.45, if dark { 65.0 } else { 52.0 }),
            done: pick("#d3d3d3", "#4d4d4d"),
            done_border: pick("#808080", "#9a9a9a"),
            crit: pick("#f04848", "#b52f2f"),
            crit_border: pick("#ff8888", "#ff8888"),
            sections: [
                (hsl(0.0, s, if dark { 45.0 } else { 70.0 }), 0.2),
                (pal.background.clone(), 0.2),
                (pick("#fff400", "#8a8400"), 0.2),
                (pal.background.clone(), 0.2),
            ],
            grid: pick("#d3d3d3", "#555555"),
            exclude: pick("#eeeeee", "#2b2b2b"),
            vert: pick("#7a00b3", "#d38cff"),
            text: pal.text_color.clone(),
            title: pal.title_color.clone(),
        }
    }
}

fn contrast_text(fill: &Color, light: &Color) -> Color {
    match color::parse(fill.css()) {
        Some(c) if c.is_dark() => Color::new("#ffffff"),
        Some(_) => Color::new("#000000"),
        None => light.clone(),
    }
}

/// Row index for every non-`vert` task (`compact` packs non-overlapping tasks of a section).
fn assign_rows(g: &Gantt, compact: bool) -> (Vec<Option<usize>>, usize) {
    let mut rows = vec![None; g.tasks.len()];
    let mut next = 0;
    for sec in 0..g.sections.len() {
        let mut lanes: Vec<i64> = Vec::new(); // lane -> busy until
        for (i, t) in g.tasks.iter().enumerate() {
            if t.section != sec || t.vert {
                continue;
            }
            let lane = if compact {
                match lanes.iter().position(|&until| until <= t.start) {
                    Some(l) => l,
                    None => {
                        lanes.push(i64::MIN);
                        lanes.len() - 1
                    }
                }
            } else {
                lanes.push(i64::MIN);
                lanes.len() - 1
            };
            lanes[lane] = t.end.max(t.start + 1);
            rows[i] = Some(next + lane);
        }
        next += lanes.len();
    }
    (rows, next)
}

/// Lay out and draw a resolved gantt chart.
pub fn draw(g: &Gantt, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let c = Colors::new(pal);
    let bar_h = cfg.section_f64("gantt", "barHeight", 20.0);
    let bar_gap = cfg.section_f64("gantt", "barGap", 4.0);
    let top_pad = cfg.section_f64("gantt", "topPadding", 50.0);
    let left_pad_cfg = cfg.section_f64("gantt", "leftPadding", 75.0);
    let right_pad = cfg.section_f64("gantt", "rightPadding", 75.0);
    let grid_pad = cfg.section_f64("gantt", "gridLineStartPadding", 35.0);
    let font = cfg.section_f64("gantt", "fontSize", 11.0);
    let sec_font = cfg.section_f64("gantt", "sectionFontSize", 11.0);
    let title_margin = cfg.section_f64("gantt", "titleTopMargin", 25.0);
    let total_w = cfg.section_f64("gantt", "useWidth", 1000.0);
    let compact = cfg
        .section("gantt", "displayMode")
        .and_then(|j| j.as_str())
        .is_some_and(|m| m == "compact");
    let top_axis = g.top_axis || cfg.section_bool("gantt", "topAxis", false);
    let axis_font = font;

    let gap = bar_h + bar_gap;
    let (rows, n_rows) = assign_rows(g, compact);
    let sec_labels: Vec<Vec<String>> = g
        .sections
        .iter()
        .map(|s| crate::text::label_lines(s))
        .collect();
    let widest_sec = sec_labels
        .iter()
        .flatten()
        .map(|l| line_width(l, sec_font, false))
        .fold(0.0, f64::max);
    let left_pad = left_pad_cfg.max(widest_sec + 20.0);
    let inner = (total_w - 2.0 * left_pad_cfg).max(200.0);
    let w = left_pad + inner + right_pad;

    let oy = if top_axis { 20.0 } else { 0.0 };
    let h = n_rows as f64 * gap + 2.0 * top_pad + oy;
    let grid_top = oy + top_pad + grid_pad - 50.0;
    let axis_y = h - 50.0;
    let row_y = |r: usize| oy + r as f64 * gap + top_pad;

    let t0 = g.tasks.iter().map(|t| t.start).min().unwrap_or(0);
    let mut t1 = g
        .tasks
        .iter()
        .map(|t| t.render_end.max(t.start))
        .max()
        .unwrap_or(DAY_MS);
    if t1 <= t0 {
        t1 = t0 + DAY_MS;
    }
    let x_of = |t: i64| left_pad + (t - t0) as f64 / (t1 - t0) as f64 * inner;

    let mut bounds = Bounds::default();
    bounds.add_rect(Rect::new(0.0, 0.0, w, h));
    let mut bands = Vec::new();
    let mut excl = Vec::new();
    let mut grid = Vec::new();
    let mut bars = Vec::new();
    let mut labels = Vec::new();
    let mut overlay = Vec::new();

    // Section bands, one per row.
    let mut row_section = vec![0usize; n_rows];
    for (t, r) in g.tasks.iter().zip(&rows) {
        if let Some(r) = r {
            row_section[*r] = t.section;
        }
    }
    for (r, &sec) in row_section.iter().enumerate() {
        let (fill, op) = &c.sections[sec % 4];
        bands.push(Element::classed_shape(
            Geometry::Rect {
                rect: Rect::new(0.0, row_y(r) - 2.0, w - right_pad / 2.0, gap),
                rx: 0.0,
            },
            Paint::fill(fill).with_opacity(*op),
            format!("section section{}", sec % 4),
        ));
    }

    // Excluded days (skipped for very long ranges, as mermaid does).
    if !g.excludes.is_empty() && t1 - t0 <= 5 * 365 * DAY_MS {
        let mut day = t0.div_euclid(DAY_MS);
        while day * DAY_MS < t1 {
            let t = day * DAY_MS;
            if is_excluded(g, t) {
                let x0 = x_of(t.max(t0));
                let x1 = x_of((t + DAY_MS).min(t1));
                excl.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: Rect::new(x0, grid_top, x1 - x0, axis_y - grid_top),
                        rx: 0.0,
                    },
                    Paint::fill(&c.exclude),
                    "exclude-range",
                ));
            }
            day += 1;
        }
    }

    // Grid + axis labels.
    let (step, unit) = g
        .tick_interval
        .unwrap_or_else(|| auto_interval(t0, t1, 10.0));
    let axis_format = g
        .axis_format
        .clone()
        .or_else(|| {
            cfg.section("gantt", "axisFormat")
                .and_then(|j| j.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "%Y-%m-%d".into());
    let mut tick_list = ticks(t0, t1, step, unit, g.week_start);
    if tick_list.len() > 200 {
        let (s2, u2) = auto_interval(t0, t1, 10.0);
        tick_list = ticks(t0, t1, s2, u2, g.week_start);
    }
    for &t in &tick_list {
        let x = x_of(t);
        grid.push(Element::classed_shape(
            Geometry::Path(vec![
                geom::PathCmd::MoveTo(Point::new(x, grid_top)),
                geom::PathCmd::LineTo(Point::new(x, axis_y)),
            ]),
            Paint::stroke(&c.grid, 1.0).with_opacity(0.8),
            "tick",
        ));
        let label = format_time(t, &axis_format);
        let lw = line_width(&label, axis_font, false);
        let mut put = |y: f64| {
            bounds.add_rect(Rect::new(x - lw / 2.0, y - axis_font, lw, axis_font * 1.3));
            grid.push(Element::classed_text(
                Text {
                    x,
                    y,
                    lines: vec![label.clone()],
                    line_height: axis_font * 1.2,
                    font_size: axis_font,
                    bold: false,
                    anchor: Anchor::Middle,
                    fill: c.text.clone(),
                },
                "tick-label",
            ));
        };
        put(axis_y + 3.0 + axis_font);
        if top_axis {
            put(grid_top - 6.0);
        }
    }

    // Tasks.
    let text_y = |r: usize| row_y(r) + bar_h / 2.0 + font * 0.35;
    for (t, r) in g.tasks.iter().zip(&rows) {
        let Some(r) = *r else { continue };
        let y = row_y(r);
        let (mut fill, mut stroke) = if t.done {
            (c.done.clone(), c.done_border.clone())
        } else if t.active {
            (c.active.clone(), c.active_border.clone())
        } else {
            (c.task.clone(), c.task_border.clone())
        };
        let mut sw = 1.0;
        if t.crit {
            stroke = c.crit_border.clone();
            sw = 2.0;
            if !t.done && !t.active {
                fill = c.crit.clone();
            }
        }
        let paint = Paint::both(&fill, &stroke, sw);
        let label = normalize_label(&t.label);
        let tw = line_width(&label, font, false);
        let (bx0, bx1) = if t.milestone {
            let mid = (x_of(t.start) + x_of(t.render_end.max(t.start))) / 2.0;
            let hs = bar_h / 2.0;
            let cy = y + bar_h / 2.0;
            bars.push(Element::classed_shape(
                Geometry::Path(geom::polygon(&[
                    Point::new(mid, cy - hs),
                    Point::new(mid + hs, cy),
                    Point::new(mid, cy + hs),
                    Point::new(mid - hs, cy),
                ])),
                paint,
                "milestone",
            ));
            (mid - hs, mid + hs)
        } else {
            let x0 = x_of(t.start);
            let x1 = x_of(t.render_end.max(t.start));
            bars.push(Element::classed_shape(
                Geometry::Rect {
                    rect: Rect::new(x0, y, (x1 - x0).max(0.0), bar_h),
                    rx: 3.0,
                },
                paint,
                "task",
            ));
            (x0, x1)
        };
        let inside = !t.milestone && tw + 6.0 <= bx1 - bx0;
        let (x, anchor, fill_text) = if inside {
            (
                (bx0 + bx1) / 2.0,
                Anchor::Middle,
                contrast_text(&fill, &c.text),
            )
        } else if bx1 + 5.0 + tw <= w || bx0 - 5.0 - tw < left_pad {
            (bx1 + 5.0, Anchor::Start, c.text.clone())
        } else {
            (bx0 - 5.0, Anchor::End, c.text.clone())
        };
        let lx = match anchor {
            Anchor::Start => x,
            Anchor::Middle => x - tw / 2.0,
            Anchor::End => x - tw,
        };
        bounds.add_rect(Rect::new(lx, y, tw, bar_h));
        labels.push(Element::classed_text(
            Text {
                x,
                y: text_y(r),
                lines: vec![label],
                line_height: font * 1.2,
                font_size: font,
                bold: false,
                anchor,
                fill: fill_text,
            },
            if inside {
                "taskText"
            } else {
                "taskTextOutside"
            },
        ));
    }

    // Section titles, centred on their rows.
    for (sec, lines) in sec_labels.iter().enumerate() {
        let rs: Vec<usize> = g
            .tasks
            .iter()
            .zip(&rows)
            .filter(|(t, _)| t.section == sec)
            .filter_map(|(_, r)| *r)
            .collect();
        let (Some(lo), Some(hi)) = (rs.iter().min(), rs.iter().max()) else {
            continue;
        };
        if lines.iter().all(String::is_empty) {
            continue;
        }
        let cy = (row_y(*lo) - 2.0 + row_y(*hi) - 2.0 + gap) / 2.0;
        let t = Text::centered(
            Point::new(10.0, cy),
            lines,
            sec_font,
            sec_font * 1.3,
            &c.text,
        )
        .anchored(Anchor::Start, 10.0);
        labels.push(Element::classed_text(t, "sectionTitle"));
    }

    // Vertical markers.
    for t in g.tasks.iter().filter(|t| t.vert) {
        let x = x_of(t.start);
        grid.push(Element::classed_shape(
            Geometry::Path(vec![
                geom::PathCmd::MoveTo(Point::new(x, grid_top)),
                geom::PathCmd::LineTo(Point::new(x, axis_y)),
            ]),
            Paint::stroke(&c.vert, 2.0).dashed(vec![6.0, 3.0]),
            "vert",
        ));
        let label = normalize_label(&t.label);
        let tw = line_width(&label, font, true);
        let y = axis_y + 3.0 + axis_font + 16.0;
        bounds.add_rect(Rect::new(x - tw / 2.0, y - font, tw, font * 1.4));
        overlay.push(Element::classed_text(
            Text {
                x,
                y,
                lines: vec![label],
                line_height: font * 1.2,
                font_size: font,
                bold: true,
                anchor: Anchor::Middle,
                fill: c.vert.clone(),
            },
            "vertText",
        ));
    }

    if let Some(title) = g.title.as_ref().filter(|t| !t.trim().is_empty()) {
        let title = normalize_label(title);
        let tw = line_width(&title, 18.0, false);
        bounds.add_rect(Rect::new(w / 2.0 - tw / 2.0, title_margin - 18.0, tw, 24.0));
        overlay.push(Element::classed_text(
            Text {
                x: w / 2.0,
                y: title_margin,
                lines: vec![title],
                line_height: 22.0,
                font_size: 18.0,
                bold: false,
                anchor: Anchor::Middle,
                fill: c.title.clone(),
            },
            "titleText",
        ));
    }

    let els = vec![
        Element::group("sections", bands),
        Element::group("excludes", excl),
        Element::group("grid", grid),
        Element::group("tasks", bars),
        Element::group("labels", labels),
        Element::group("overlay", overlay),
    ];
    Scene {
        kind: DiagramKind::Gantt,
        view_box: bounds.rect().pad(8.0),
        elements: els,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// Parse and render a `gantt` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let g = parse(body, title)?;
    Ok(draw(&g, cfg, pal))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i64, m: u32, day: u32) -> i64 {
        days_from_civil(y, m, day) * DAY_MS
    }

    #[test]
    fn civil_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        for z in [-1_000_000i64, -1, 0, 59, 365, 19_000, 2_932_896] {
            let (y, m, dd) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, dd), z);
        }
        assert_eq!(weekday(d(2024, 1, 1)), 1); // Monday
        assert_eq!(weekday(d(1970, 1, 1)), 4); // Thursday
    }

    #[test]
    fn month_arithmetic_clamps() {
        assert_eq!(add_months(d(2024, 1, 31), 1), d(2024, 2, 29));
        assert_eq!(add_months(d(2023, 1, 31), 1), d(2023, 2, 28));
        assert_eq!(add_months(d(2023, 11, 15), 3), d(2024, 2, 15));
        assert_eq!(add_months(d(2024, 3, 15), -3), d(2023, 12, 15));
    }

    #[test]
    fn durations() {
        let s = d(2024, 1, 1);
        assert_eq!(parse_duration("3d").unwrap().add_to(s), d(2024, 1, 4));
        assert_eq!(parse_duration("1w").unwrap().add_to(s), d(2024, 1, 8));
        assert_eq!(parse_duration("12h").unwrap().add_to(s), s + 12 * HOUR_MS);
        assert_eq!(parse_duration("30m").unwrap().add_to(s), s + 30 * MINUTE_MS);
        assert_eq!(parse_duration("1.5d").unwrap().add_to(s), s + 36 * HOUR_MS);
        assert_eq!(parse_duration("2M").unwrap().add_to(s), d(2024, 3, 1));
        assert_eq!(parse_duration("1y").unwrap().add_to(s), d(2025, 1, 1));
        assert_eq!(parse_duration("250ms").unwrap().add_to(s), s + 250);
        assert!(parse_duration("3x").is_none());
        assert!(parse_duration("d").is_none());
        assert!(parse_duration("2024-01-01").is_none());
    }

    #[test]
    fn date_formats() {
        assert_eq!(parse_date("2024-02-29", "YYYY-MM-DD"), Some(d(2024, 2, 29)));
        assert_eq!(parse_date("2023-02-29", "YYYY-MM-DD"), None);
        assert_eq!(
            parse_date("2024-01-02 13:45", "YYYY-MM-DD HH:mm"),
            Some(d(2024, 1, 2) + 13 * HOUR_MS + 45 * MINUTE_MS)
        );
        assert_eq!(parse_date("15-03-2024", "DD-MM-YYYY"), Some(d(2024, 3, 15)));
        assert_eq!(parse_date("1700000000", "X"), Some(1_700_000_000_000));
        assert_eq!(parse_date("1700000000123", "x"), Some(1_700_000_000_123));
        assert_eq!(parse_date("Mar 5th 24", "MMM Do YY"), Some(d(2024, 3, 5)));
        assert_eq!(
            parse_date("05/03/2024 1:30 PM", "DD/MM/YYYY h:mm A"),
            Some(d(2024, 3, 5) + 13 * HOUR_MS + 30 * MINUTE_MS)
        );
        assert_eq!(
            parse_date("17:49", "HH:mm"),
            Some(17 * HOUR_MS + 49 * MINUTE_MS)
        );
        assert_eq!(parse_date("2024-01-01", "DD-MM-YYYY"), None);
        assert_eq!(
            parse_iso("2024-01-01T10:00"),
            Some(d(2024, 1, 1) + 10 * HOUR_MS)
        );
    }

    #[test]
    fn strftime() {
        let t = d(2024, 3, 5) + 9 * HOUR_MS + 7 * MINUTE_MS;
        assert_eq!(format_time(t, "%Y-%m-%d"), "2024-03-05");
        assert_eq!(format_time(t, "%b %e"), "Mar  5");
        assert_eq!(format_time(t, "%-d %B %y"), "5 March 24");
        assert_eq!(format_time(t, "%H:%M %a %A"), "09:07 Tue Tuesday");
        assert_eq!(format_time(t, "%I %p %j %%"), "09 AM 065 %");
    }

    #[test]
    fn tick_selection() {
        let t0 = d(2024, 1, 1);
        assert_eq!(auto_interval(t0, d(2024, 1, 11), 10.0), (1, TickUnit::Day));
        assert_eq!(auto_interval(t0, d(2024, 3, 15), 10.0), (1, TickUnit::Week));
        assert_eq!(auto_interval(t0, d(2025, 1, 1), 10.0), (1, TickUnit::Month));
        assert_eq!(
            auto_interval(t0, t0 + 10 * HOUR_MS, 10.0),
            (1, TickUnit::Hour)
        );
        let w = ticks(t0, d(2024, 1, 31), 1, TickUnit::Week, 0);
        assert_eq!(w[0], d(2024, 1, 7));
        assert!(w.iter().all(|&t| weekday(t) == 0));
        let m = ticks(t0, d(2024, 12, 31), 3, TickUnit::Month, 0);
        assert_eq!(m, vec![t0, d(2024, 4, 1), d(2024, 7, 1), d(2024, 10, 1)]);
        assert_eq!(parse_tick_interval("2week"), Some((2, TickUnit::Week)));
        assert_eq!(parse_tick_interval("0day"), None);
    }

    const SAMPLE: &str = "gantt
    title A Gantt Diagram
    dateFormat YYYY-MM-DD
    section Section
    A task           :a1, 2014-01-01, 30d
    Another task     :after a1, 20d
    section Another
    Task in sec      :2014-01-12, 12d
    another task     :24d
    Done task        :done, crit, des1, 2014-01-06, 2014-01-08
    Milestone        :milestone, m1, 2014-01-25, 0d
    Until task       :u1, 2014-01-02, until des1
";

    #[test]
    fn parses_and_resolves_tasks() {
        let g = parse(SAMPLE, None).unwrap();
        assert_eq!(g.title.as_deref(), Some("A Gantt Diagram"));
        assert_eq!(g.sections, vec!["Section", "Another"]);
        let t = &g.tasks;
        assert_eq!(t[0].id, "a1");
        assert_eq!(t[0].end, d(2014, 1, 31));
        assert_eq!(t[1].start, d(2014, 1, 31));
        assert_eq!(t[1].end, d(2014, 2, 20));
        assert_eq!(t[1].id, "task2");
        assert_eq!(t[3].start, d(2014, 1, 24)); // previous task's end
        assert!(t[4].done && t[4].crit && !t[4].active);
        assert_eq!(t[4].end, d(2014, 1, 8));
        assert!(t[5].milestone);
        assert_eq!(t[6].end, d(2014, 1, 6));
        assert_eq!(t[2].section, 1);
    }

    #[test]
    fn excludes_extend_tasks() {
        let src = "gantt\ndateFormat YYYY-MM-DD\nexcludes weekends, 2024-01-10\nsection S\nA :a, 2024-01-04, 3d\nB :after a, 1d";
        let g = parse(src, None).unwrap();
        // Thu 4 + 3 working days (Thu, Fri, Mon) => ends Tue 9.
        assert_eq!(g.tasks[0].end, d(2024, 1, 9));
        // Wed 10 is excluded, so the 1-day task runs through Thu 11.
        assert_eq!(g.tasks[1].start, d(2024, 1, 9));
        assert_eq!(g.tasks[1].end, d(2024, 1, 10) + DAY_MS);
    }

    #[test]
    fn inclusive_end_dates_and_until_forward_refs() {
        let src = "gantt\ninclusiveEndDates\nA :a, until b\nB :b, 2024-01-05, 2024-01-06";
        assert!(parse(src, None).is_err()); // "a" is not a valid start date
        let src = "gantt\ninclusiveEndDates\nB :b, 2024-01-05, 2024-01-06\nA :a, 2024-01-01, until c\nC :c, after b, 1d";
        let g = parse(src, None).unwrap();
        assert_eq!(g.tasks[0].end, d(2024, 1, 7));
        assert_eq!(g.tasks[1].end, d(2024, 1, 7));
    }

    #[test]
    fn parse_errors_have_line_numbers() {
        let e = parse("gantt\ndateFormat YYYY-MM-DD\n\nA : a1, nope, 3d", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 4, .. }), "{e}");
        let e = parse("gantt\nsection S\njust words", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        let e = parse("gantt\nA :a, after zz, 1d", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        let e = parse("gantt\nA :a, after b, 1d\nB :b, after a, 1d", None).unwrap_err();
        assert!(e.to_string().contains("Circular"));
        let e = parse("gantt\nA : 3d", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        assert!(parse("pie\n", None).is_err());
        assert!(parse("gantt\ntickInterval 3fortnight", None).is_err());
    }

    #[test]
    fn renders_deterministically() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let a = render(SAMPLE, None, &cfg, &pal).unwrap();
        let b = render(SAMPLE, None, &cfg, &pal).unwrap();
        assert_eq!(a, b);
        assert!(a.element_count() > 20);
        assert!(a.view_box.w > 900.0);
    }

    #[test]
    fn compact_mode_shares_rows() {
        let g = parse(
            "gantt\nsection S\nA :a, 2024-01-01, 2d\nB :b, 2024-01-03, 2d\nC :c, 2024-01-02, 2d",
            None,
        )
        .unwrap();
        assert_eq!(assign_rows(&g, false).1, 3);
        let (rows, n) = assign_rows(&g, true);
        assert_eq!(n, 2);
        assert_eq!(rows, vec![Some(0), Some(0), Some(1)]);
    }
}
