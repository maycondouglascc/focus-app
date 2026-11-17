//! Time, duration, date, clock-input, and currency formatting matching
//! Electron `format.ts` and Windows locale preferences.
#![allow(dead_code)] // wired across F3 screens in subsequent commits

use crate::time::{at_minutes, days_in_month, from_local, to_local, DAY, HOUR, MIN};

const MONTHS_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTHS_LONG: [&str; 12] = [
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
const WEEKDAYS_SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

pub fn weekday_short(weekday_mon0: u8) -> &'static str {
    WEEKDAYS_SHORT[(weekday_mon0 as usize) % 7]
}

pub fn month_short(month1: u8) -> &'static str {
    MONTHS_SHORT[(month1.saturating_sub(1) as usize) % 12]
}

pub fn month_long(month1: u8) -> &'static str {
    MONTHS_LONG[(month1.saturating_sub(1) as usize) % 12]
}

/// Inserts `sep` every 3 digits in a non-negative integer.
pub fn group_int(n: i64, sep: char) -> String {
    let neg = n < 0;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + usize::from(neg));
    if neg {
        out.push('-');
    }
    let len = digits.len();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(ch);
    }
    out
}

/// `1h 30m` · `45m` · `0m` · `5,228h` (past 100 hours, rounded to whole hours).
pub fn duration(ms: i64) -> String {
    let ms = ms.max(0);
    let m = ((ms as f64) / (MIN as f64)).round() as i64;
    let h = m / 60;
    let r = m % 60;
    if h == 0 {
        return format!("{r}m");
    }
    if h >= 100 {
        let whole_h = ((ms as f64) / (HOUR as f64)).round() as i64;
        return format!("{}h", group_int(whole_h, ','));
    }
    if r != 0 {
        format!("{h}h {r}m")
    } else {
        format!("{h}h")
    }
}

/// Under a minute reads `<1m`, not `0m`.
pub fn short_duration(ms: i64) -> String {
    if ms < MIN {
        "<1m".into()
    } else {
        duration(ms)
    }
}

/// `1:30` — compact hours:minutes for tight spots and totals.
pub fn hm(ms: i64) -> String {
    let m = ((ms.max(0) as f64) / (MIN as f64)).round() as i64;
    format!("{}:{:02}", m / 60, m % 60)
}

/// `1:05:09` — the running timer.
pub fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// `1.5h` · `2h`
pub fn hours(ms: i64, digits: usize) -> String {
    let v = (ms.max(0) as f64) / (HOUR as f64);
    let raw = format!("{v:.digits$}");
    let trimmed = if raw.contains('.') {
        raw.trim_end_matches('0').trim_end_matches('.')
    } else {
        raw.as_str()
    };
    format!("{trimmed}h")
}

/// `09:30` (24h) or `9:30 AM` (12h).
pub fn time(epoch_ms: i64, is_24h: bool) -> String {
    let l = to_local(epoch_ms);
    if is_24h {
        format!("{:02}:{:02}", l.hour, l.minute)
    } else {
        let suffix = if l.hour >= 12 { "PM" } else { "AM" };
        let h12 = match l.hour % 12 {
            0 => 12,
            h => h,
        };
        format!("{h12}:{:02} {suffix}", l.minute)
    }
}

/// `Mon, Nov 16`
pub fn day_label(epoch_ms: i64) -> String {
    let l = to_local(epoch_ms);
    format!(
        "{}, {} {}",
        weekday_short(l.weekday_mon0()),
        month_short(l.month),
        l.day
    )
}

/// `Nov 16`
pub fn short_day(epoch_ms: i64) -> String {
    let l = to_local(epoch_ms);
    format!("{} {}", month_short(l.month), l.day)
}

/// `Nov 16, 2026` / `Nov 16 – 22, 2026` / `Nov 28 – Dec 4, 2026` / `Dec 28, 2026 – Jan 3, 2027`
pub fn range_label(from: i64, to_exclusive: i64) -> String {
    let a = to_local(from);
    let b = to_local(to_exclusive.saturating_sub(1).max(from));
    let same_year = a.year == b.year;
    let same_month = same_year && a.month == b.month;
    if same_month && a.day == b.day {
        format!("{} {}, {}", month_short(a.month), a.day, a.year)
    } else if same_month {
        format!("{} {} – {}, {}", month_short(a.month), a.day, b.day, a.year)
    } else if same_year {
        format!(
            "{} {} – {} {}, {}",
            month_short(a.month),
            a.day,
            month_short(b.month),
            b.day,
            a.year
        )
    } else {
        format!(
            "{} {}, {} – {} {}, {}",
            month_short(a.month),
            a.day,
            a.year,
            month_short(b.month),
            b.day,
            b.year
        )
    }
}

/// Formats rounded integer currency amounts (`maximumFractionDigits: 0`).
pub fn money(amount: f64, currency: &str) -> String {
    let rounded = amount.round() as i64;
    let sign = if rounded < 0 { "-" } else { "" };
    let abs = rounded.abs();
    let code = currency.trim().to_ascii_uppercase();
    match code.as_str() {
        "USD" => format!("{sign}${}", group_int(abs, ',')),
        "BRL" => format!("{sign}R$ {}", group_int(abs, '.')),
        "EUR" => format!("{sign}€{}", group_int(abs, ',')),
        "GBP" => format!("{sign}£{}", group_int(abs, ',')),
        "CHF" => format!("{sign}CHF {}", group_int(abs, '\'')),
        "PLN" => format!("{sign}{} zł", group_int(abs, ' ')),
        "SEK" | "NOK" | "DKK" => format!("{sign}{} kr", group_int(abs, ' ')),
        "CZK" => format!("{sign}{} Kč", group_int(abs, ' ')),
        "" => format!("{sign}{}", group_int(abs, ',')),
        other => format!("{sign}{other} {}", group_int(abs, ',')),
    }
}

/// `"09:30"` -> `570`. Accepts `"9"`, `"930"`, `"9:30"`, `"9.30"`, `"21:05"`, `"24:00"`.
pub fn parse_clock(s: &str) -> Option<u32> {
    let t = s.trim().replace('.', ":");
    if t.is_empty() {
        return None;
    }
    let (h, m): (u32, u32) = if !t.contains(':') {
        if !t.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        match t.len() {
            1 | 2 => (t.parse().ok()?, 0),
            3 | 4 => {
                let split = t.len() - 2;
                (t[..split].parse().ok()?, t[split..].parse().ok()?)
            }
            _ => return None,
        }
    } else {
        let mut parts = t.split(':');
        let lhs = parts.next()?;
        let rhs = parts.next()?;
        if parts.next().is_some()
            || lhs.is_empty()
            || lhs.len() > 2
            || rhs.is_empty()
            || rhs.len() > 2
            || !lhs.chars().all(|c| c.is_ascii_digit())
            || !rhs.chars().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        (lhs.parse().ok()?, rhs.parse().ok()?)
    };
    if h > 24 || m > 59 || (h == 24 && m > 0) {
        return None;
    }
    Some(h * 60 + m)
}

pub fn clock_of(minutes: u32) -> String {
    let clamped = minutes.min(24 * 60);
    format!("{:02}:{:02}", clamped / 60, clamped % 60)
}

pub fn to_date_input(epoch_ms: i64) -> String {
    let l = to_local(epoch_ms);
    format!("{:04}-{:02}-{:02}", l.year, l.month, l.day)
}

pub fn from_date_input(s: &str) -> Option<i64> {
    let mut parts = s.trim().split('-');
    let y: i32 = parts.next()?.parse().ok()?;
    let m: u8 = parts.next()?.parse().ok()?;
    let d: u8 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1970..=2100).contains(&y) || !(1..=12).contains(&m) {
        return None;
    }
    if d < 1 || d > days_in_month(y, m) {
        return None;
    }
    Some(from_local(y, m, d, 0, 0, 0, 0))
}

/// Formats `"YYYY-MM-DD"` in CSV rows.
pub fn csv_date(epoch_ms: i64) -> String {
    to_date_input(epoch_ms)
}

/// Computes local day + minutes timestamp helper for tests & screens.
pub fn local_day_at(year: i32, month: u8, day: u8, minutes: i32) -> i64 {
    at_minutes(from_local(year, month, day, 0, 0, 0, 0), minutes)
}

/// Checks if two timestamps fall in the same local calendar day.
pub fn same_local_day(a: i64, b: i64) -> bool {
    let la = to_local(a);
    let lb = to_local(b);
    (la.year, la.month, la.day) == (lb.year, lb.month, lb.day)
}

pub const ALL_CURRENCIES: [&str; 10] = [
    "BRL", "USD", "EUR", "GBP", "PLN", "CHF", "SEK", "NOK", "DKK", "CZK",
];

/// Splits a comma- or newline-separated exclusion list into trimmed unique non-empty items.
pub fn parse_csv_tags(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in raw.split([',', '\n', ';']) {
        let trimmed = part.trim();
        if !trimmed.is_empty() && !out.iter().any(|s: &String| s.eq_ignore_ascii_case(trimmed)) {
            out.push(trimmed.to_string());
        }
    }
    out
}

/// Unused `DAY` silence check helper if needed externally; keep constants accessible.
pub const _DAY_CHECK: i64 = DAY;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_clocks_match_electron_formatting() {
        assert_eq!(duration(0), "0m");
        assert_eq!(duration(45 * MIN), "45m");
        assert_eq!(duration(90 * MIN), "1h 30m");
        assert_eq!(duration(2 * HOUR), "2h");
        assert_eq!(duration(5_228 * HOUR), "5,228h");
        assert_eq!(short_duration(25_000), "<1m");
        assert_eq!(short_duration(60_000), "1m");
        assert_eq!(hm(90 * MIN), "1:30");
        assert_eq!(clock(3_909_000), "1:05:09");
        assert_eq!(hours(90 * MIN, 1), "1.5h");
        assert_eq!(hours(2 * HOUR, 1), "2h");
    }

    #[test]
    fn parse_clock_accepts_flexible_shorthand_and_rejects_invalid() {
        assert_eq!(parse_clock("9"), Some(540));
        assert_eq!(parse_clock("930"), Some(570));
        assert_eq!(parse_clock("09:30"), Some(570));
        assert_eq!(parse_clock("9.30"), Some(570));
        assert_eq!(parse_clock("21:05"), Some(21 * 60 + 5));
        assert_eq!(parse_clock("24:00"), Some(24 * 60));
        assert_eq!(parse_clock("24:01"), None);
        assert_eq!(parse_clock("9:60"), None);
        assert_eq!(parse_clock("abc"), None);
        assert_eq!(clock_of(570), "09:30");
    }

    #[test]
    fn money_and_dates_format_cleanly() {
        assert_eq!(money(3958.0, "USD"), "$3,958");
        assert_eq!(money(1250.4, "BRL"), "R$ 1.250");
        assert_eq!(money(500.0, "PLN"), "500 zł");

        let d = from_date_input("2026-11-17").unwrap();
        assert_eq!(to_date_input(d), "2026-11-17");
        assert_eq!(from_date_input("2026-02-30"), None);

        let a = from_local(2026, 11, 16, 0, 0, 0, 0);
        let b = from_local(2026, 11, 23, 0, 0, 0, 0);
        assert_eq!(range_label(a, b), "Nov 16 – 22, 2026");
        assert_eq!(
            parse_csv_tags("1Password, bitwarden , 1password , , KeePass "),
            vec!["1Password", "bitwarden", "KeePass"]
        );
    }
}
