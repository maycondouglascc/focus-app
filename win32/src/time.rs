//! DST-aware local calendar and time calculations using Win32 timezone
//! conversions (`SystemTimeToTzSpecificLocalTime` / `TzSpecificLocalTimeToSystemTime`),
//! matching the Electron `time.ts` contract for day/week/month/year boundaries
//! including 23-hour and 25-hour DST transition days.

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    TzSpecificLocalTimeToSystemTime, TIME_ZONE_INFORMATION,
};

pub const MIN: i64 = 60_000;
pub const HOUR: i64 = 60 * MIN;
pub const DAY: i64 = 24 * HOUR;

/// 100-ns ticks between 1601-01-01 (Win32 FILETIME epoch) and 1970-01-01 (Unix epoch).
const UNIX_EPOCH_FT: i64 = 116_444_736_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalDateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub ms: u16,
    /// `0 = Sunday .. 6 = Saturday`, matching Win32 `SYSTEMTIME::wDayOfWeek`.
    pub day_of_week: u8,
}

impl LocalDateTime {
    /// Monday-first day of week (`0 = Mon .. 6 = Sun`).
    pub fn weekday_mon0(self) -> u8 {
        (self.day_of_week + 6) % 7
    }
}

/// Howard Hinnant's O(1) civil-calendar conversion: days since 1970-01-01.
pub fn days_from_civil(y: i32, m: u8, d: u8) -> i64 {
    let mut y = y as i64;
    let m = m as i64;
    let d = d as i64;
    y -= i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of `days_from_civil`: `(year, month 1..=12, day 1..=31)`.
pub fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + i64::from(m <= 2);
    (y as i32, m as u8, d as u8)
}

pub fn days_in_month(y: i32, m: u8) -> u8 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn epoch_ms_to_utc_st(epoch_ms: i64) -> Option<SYSTEMTIME> {
    let ticks = epoch_ms
        .checked_mul(10_000)?
        .checked_add(UNIX_EPOCH_FT)?
        .max(0) as u64;
    let ft = FILETIME {
        dwLowDateTime: (ticks & 0xFFFF_FFFF) as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut st = SYSTEMTIME::default();
    // SAFETY: `ft` and `st` are valid stack structs.
    unsafe {
        FileTimeToSystemTime(&ft, &mut st).ok()?;
    }
    Some(st)
}

fn utc_st_to_epoch_ms(st: &SYSTEMTIME) -> Option<i64> {
    let mut ft = FILETIME::default();
    // SAFETY: `st` and `ft` are valid stack structs.
    unsafe {
        SystemTimeToFileTime(st, &mut ft).ok()?;
    }
    let ticks = ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
    let ms = (ticks as i64 - UNIX_EPOCH_FT).div_euclid(10_000);
    Some(ms)
}

pub fn to_local_tz(epoch_ms: i64, tz: Option<&TIME_ZONE_INFORMATION>) -> LocalDateTime {
    let Some(utc_st) = epoch_ms_to_utc_st(epoch_ms) else {
        return LocalDateTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            ms: 0,
            day_of_week: 4,
        };
    };
    let mut local_st = SYSTEMTIME::default();
    let tz_ptr = tz.map_or(std::ptr::null(), std::ptr::from_ref);
    // SAFETY: `tz_ptr` is either null (current system TZ) or a valid reference; `utc_st` and `local_st` are valid.
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(Some(tz_ptr), &utc_st, &mut local_st) };
    let st = if ok.is_ok() { local_st } else { utc_st };
    LocalDateTime {
        year: st.wYear as i32,
        month: st.wMonth as u8,
        day: st.wDay as u8,
        hour: st.wHour as u8,
        minute: st.wMinute as u8,
        second: st.wSecond as u8,
        ms: st.wMilliseconds,
        day_of_week: st.wDayOfWeek as u8,
    }
}

pub fn to_local(epoch_ms: i64) -> LocalDateTime {
    to_local_tz(epoch_ms, None)
}

#[allow(clippy::too_many_arguments)]
fn raw_from_local_tz(
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    ms: u16,
    tz: Option<&TIME_ZONE_INFORMATION>,
) -> Option<i64> {
    let local_st = SYSTEMTIME {
        wYear: year.clamp(1601, 9999) as u16,
        wMonth: month.clamp(1, 12) as u16,
        wDay: day.clamp(1, 31) as u16,
        wHour: hour.min(23) as u16,
        wMinute: minute.min(59) as u16,
        wSecond: second.min(59) as u16,
        wMilliseconds: ms.min(999),
        wDayOfWeek: 0,
    };
    let mut utc_st = SYSTEMTIME::default();
    let tz_ptr = tz.map_or(std::ptr::null(), std::ptr::from_ref);
    // SAFETY: `tz_ptr` is null or valid reference; `local_st` and `utc_st` are stack structs.
    let ok = unsafe { TzSpecificLocalTimeToSystemTime(Some(tz_ptr), &local_st, &mut utc_st) };
    if ok.is_ok() {
        utc_st_to_epoch_ms(&utc_st)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
pub fn from_local_tz(
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    ms: u16,
    tz: Option<&TIME_ZONE_INFORMATION>,
) -> i64 {
    let target_days = days_from_civil(year, month, day);
    let mut try_hour = hour.min(23);
    while let Some(t) = raw_from_local_tz(year, month, day, try_hour, minute, second, ms, tz) {
        let back = to_local_tz(t, tz);
        if days_from_civil(back.year, back.month, back.day) >= target_days || try_hour >= 23 {
            return t;
        }
        // Midnight (or early morning) DST spring-forward rolled back into the previous day;
        // advance by an hour to match JS `Date.setHours(0)` landing on 01:00.
        try_hour += 1;
    }
    // Fallback to pure civil UTC if out of Win32 SYSTEMTIME range.
    target_days * DAY
        + i64::from(hour) * HOUR
        + i64::from(minute) * MIN
        + i64::from(second) * 1000
        + i64::from(ms)
}

#[allow(clippy::too_many_arguments)]
pub fn from_local(year: i32, month: u8, day: u8, hour: u8, minute: u8, second: u8, ms: u16) -> i64 {
    from_local_tz(year, month, day, hour, minute, second, ms, None)
}

pub fn start_of_day_tz(t: i64, tz: Option<&TIME_ZONE_INFORMATION>) -> i64 {
    let l = to_local_tz(t, tz);
    from_local_tz(l.year, l.month, l.day, 0, 0, 0, 0, tz)
}

pub fn start_of_day(t: i64) -> i64 {
    start_of_day_tz(t, None)
}

pub fn add_days_tz(t: i64, n: i32, tz: Option<&TIME_ZONE_INFORMATION>) -> i64 {
    let l = to_local_tz(t, tz);
    let (y, m, d) = civil_from_days(days_from_civil(l.year, l.month, l.day) + i64::from(n));
    from_local_tz(y, m, d, l.hour, l.minute, l.second, l.ms, tz)
}

pub fn add_days(t: i64, n: i32) -> i64 {
    add_days_tz(t, n, None)
}

/// Shifts `t` by `n` local calendar days and returns the start of that day.
pub fn add_days_start_tz(t: i64, n: i32, tz: Option<&TIME_ZONE_INFORMATION>) -> i64 {
    let l = to_local_tz(t, tz);
    let (y, m, d) = civil_from_days(days_from_civil(l.year, l.month, l.day) + i64::from(n));
    from_local_tz(y, m, d, 0, 0, 0, 0, tz)
}

/// Shifts `t` by `n` local calendar days and returns the start of that day.
pub fn add_days_start(t: i64, n: i32) -> i64 {
    add_days_start_tz(t, n, None)
}

/// Week start aligned to `first_day_mon0` (`0 = Monday`, `6 = Sunday`).
pub fn start_of_week_with_first_tz(
    t: i64,
    first_day_mon0: u8,
    tz: Option<&TIME_ZONE_INFORMATION>,
) -> i64 {
    let day0 = start_of_day_tz(t, tz);
    let dow = to_local_tz(day0, tz).weekday_mon0();
    let offset = (dow + 7 - (first_day_mon0 % 7)) % 7;
    add_days_start_tz(day0, -i32::from(offset), tz)
}

/// Week start aligned to `first_day_mon0` (`0 = Monday`, `6 = Sunday`).
pub fn start_of_week_with_first(t: i64, first_day_mon0: u8) -> i64 {
    start_of_week_with_first_tz(t, first_day_mon0, None)
}

/// Weeks start on Monday by default (matching Electron `startOfWeek`).
pub fn start_of_week(t: i64) -> i64 {
    start_of_week_with_first(t, 0)
}

pub fn start_of_month(t: i64) -> i64 {
    let l = to_local(t);
    from_local(l.year, l.month, 1, 0, 0, 0, 0)
}

pub fn add_months(t: i64, n: i32) -> i64 {
    let l = to_local(t);
    let total_months = (l.year * 12 + i32::from(l.month) - 1) + n;
    let ny = total_months.div_euclid(12);
    let nm = (total_months.rem_euclid(12) + 1) as u8;
    let nd = l.day.min(days_in_month(ny, nm));
    from_local(ny, nm, nd, l.hour, l.minute, l.second, l.ms)
}

pub fn start_of_year(t: i64) -> i64 {
    let l = to_local(t);
    from_local(l.year, 1, 1, 0, 0, 0, 0)
}

/// Minutes since local midnight of the day `t` falls in (DST-safe for layout).
pub fn minutes_of_day_tz(t: i64, tz: Option<&TIME_ZONE_INFORMATION>) -> f64 {
    let l = to_local_tz(t, tz);
    f64::from(l.hour) * 60.0 + f64::from(l.minute) + f64::from(l.second) / 60.0
}

pub fn minutes_of_day(t: i64) -> f64 {
    minutes_of_day_tz(t, None)
}

/// Local timestamp at `minutes` past midnight on `day_start` (DST-safe).
pub fn at_minutes_tz(day_start: i64, minutes: i32, tz: Option<&TIME_ZONE_INFORMATION>) -> i64 {
    let l = to_local_tz(day_start, tz);
    let base_days = days_from_civil(l.year, l.month, l.day);
    let extra_days = minutes.div_euclid(24 * 60);
    let rem = minutes.rem_euclid(24 * 60);
    let (y, m, d) = civil_from_days(base_days + i64::from(extra_days));
    from_local_tz(y, m, d, (rem / 60) as u8, (rem % 60) as u8, 0, 0, tz)
}

pub fn at_minutes(day_start: i64, minutes: i32) -> i64 {
    at_minutes_tz(day_start, minutes, None)
}

pub fn same_day(a: i64, b: i64) -> bool {
    start_of_day(a) == start_of_day(b)
}

/// The length of `[start, end)` that falls inside `[from, to)`.
pub fn overlap(start: i64, end: i64, from: i64, to: i64) -> i64 {
    (end.min(to) - start.max(from)).max(0)
}

pub fn iso_week(t: i64) -> u32 {
    let l = to_local(t);
    let thu_days = days_from_civil(l.year, l.month, l.day) + 3 - i64::from(l.weekday_mon0());
    let (ty, _, _) = civil_from_days(thu_days);
    let jan4_epoch = from_local(ty, 1, 4, 0, 0, 0, 0);
    let jan4_l = to_local(jan4_epoch);
    let jan4_thu_days = days_from_civil(ty, 1, 4) + 3 - i64::from(jan4_l.weekday_mon0());
    (1 + (thu_days - jan4_thu_days) / 7) as u32
}

/// ISO week number for a 7-day calendar week starting at `week_start`. Uses the Thursday
/// falling inside `[week_start, week_start + 7d)` so Sunday/Saturday-start weeks report the
/// ISO week containing their weekdays (e.g. Sun 2026-11-15..Sat 2026-11-21 -> W47, not W46).
pub fn week_iso_number(week_start: i64) -> u32 {
    let thu = (0..7)
        .map(|d| add_days_start(week_start, d))
        .find(|&ts| to_local(ts).weekday_mon0() == 3)
        .unwrap_or_else(|| add_days_start(week_start, 3));
    iso_week(thu)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic US Eastern-style TZ: +0h DST on 2nd Sunday of March at 02:00 (23h day),
    /// -1h standard on 1st Sunday of November at 02:00 (25h day).
    fn us_eastern_tz() -> TIME_ZONE_INFORMATION {
        TIME_ZONE_INFORMATION {
            Bias: 300, // UTC-5
            StandardDate: SYSTEMTIME {
                wYear: 0,
                wMonth: 11,
                wDayOfWeek: 0, // Sunday
                wDay: 1,       // 1st occurrence
                wHour: 2,
                ..Default::default()
            },
            StandardBias: 0,
            DaylightDate: SYSTEMTIME {
                wYear: 0,
                wMonth: 3,
                wDayOfWeek: 0, // Sunday
                wDay: 2,       // 2nd occurrence
                wHour: 2,
                ..Default::default()
            },
            DaylightBias: -60, // UTC-4
            ..Default::default()
        }
    }

    #[test]
    fn civil_roundtrip_and_leap_years() {
        for (y, m, d) in [
            (2024, 2, 29),
            (2025, 2, 28),
            (2026, 11, 17),
            (2000, 1, 1),
            (1970, 1, 1),
        ] {
            let z = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(z), (y, m, d));
        }
    }

    #[test]
    fn handles_23h_and_25h_dst_transition_days() {
        let tz = us_eastern_tz();

        // 2026-03-08 is 2nd Sunday in March: 23-hour day (02:00 jumps to 03:00).
        let spring_mid = from_local_tz(2026, 3, 8, 12, 0, 0, 0, Some(&tz));
        let spring_0 = start_of_day_tz(spring_mid, Some(&tz));
        let spring_next = add_days_tz(spring_0, 1, Some(&tz));
        assert_eq!(spring_next - spring_0, 23 * HOUR);
        assert_eq!(
            minutes_of_day_tz(
                from_local_tz(2026, 3, 8, 15, 30, 0, 0, Some(&tz)),
                Some(&tz)
            ),
            15.0 * 60.0 + 30.0
        );

        // 2026-11-01 is 1st Sunday in November: 25-hour day (01:00 repeats).
        let fall_mid = from_local_tz(2026, 11, 1, 12, 0, 0, 0, Some(&tz));
        let fall_0 = start_of_day_tz(fall_mid, Some(&tz));
        let fall_next = add_days_tz(fall_0, 1, Some(&tz));
        assert_eq!(fall_next - fall_0, 25 * HOUR);

        let end_at_17 = at_minutes_tz(fall_0, 17 * 60, Some(&tz));
        assert_eq!(minutes_of_day_tz(end_at_17, Some(&tz)), 17.0 * 60.0);
    }

    #[test]
    fn week_start_and_iso_week_match_calendar_rules() {
        // 2026-11-17 is Tuesday in ISO week 47.
        let tue = from_local(2026, 11, 17, 10, 0, 0, 0);
        assert_eq!(iso_week(tue), 47);

        let mon = start_of_week_with_first(tue, 0);
        let ml = to_local(mon);
        assert_eq!(
            (ml.year, ml.month, ml.day, ml.weekday_mon0()),
            (2026, 11, 16, 0)
        );

        let sun = start_of_week_with_first(tue, 6);
        let sl = to_local(sun);
        assert_eq!(
            (sl.year, sl.month, sl.day, sl.weekday_mon0()),
            (2026, 11, 15, 6)
        );
    }

    /// Synthetic Chile/Santiago-style TZ: UTC-4 standard, UTC-3 DST springing forward
    /// at 00:00 on the 1st Sunday of September (`00:00` does not exist).
    fn santiago_tz() -> TIME_ZONE_INFORMATION {
        TIME_ZONE_INFORMATION {
            Bias: 240, // UTC-4
            StandardDate: SYSTEMTIME {
                wYear: 0,
                wMonth: 4,
                wDayOfWeek: 0, // Sunday
                wDay: 1,       // 1st occurrence
                wHour: 0,
                ..Default::default()
            },
            StandardBias: 0,
            DaylightDate: SYSTEMTIME {
                wYear: 0,
                wMonth: 9,
                wDayOfWeek: 0, // Sunday
                wDay: 1,       // 1st occurrence (2026-09-06)
                wHour: 0,
                ..Default::default()
            },
            DaylightBias: -60, // UTC-3
            ..Default::default()
        }
    }

    #[test]
    fn midnight_spring_forward_keeps_same_local_day_and_advances_week_cleanly() {
        let tz = santiago_tz();
        // 2026-09-06 is the 1st Sunday of September: 00:00 jumps to 01:00.
        let sep6_mid = from_local_tz(2026, 9, 6, 12, 0, 0, 0, Some(&tz));
        let sep6_sod = start_of_day_tz(sep6_mid, Some(&tz));
        let l6 = to_local_tz(sep6_sod, Some(&tz));
        assert_eq!((l6.year, l6.month, l6.day, l6.hour), (2026, 9, 6, 1));

        // Stepping 7 days from Sat 2026-09-05 must produce 7 distinct days without repeating Sep 5.
        let sep5_mid = from_local_tz(2026, 9, 5, 12, 0, 0, 0, Some(&tz));
        let mut d = start_of_day_tz(sep5_mid, Some(&tz));
        let mut dates = Vec::new();
        for _ in 0..7 {
            let ld = to_local_tz(d, Some(&tz));
            dates.push((ld.month, ld.day, ld.hour));
            d = add_days_start_tz(d, 1, Some(&tz));
        }
        assert_eq!(
            dates,
            [
                (9, 5, 0),
                (9, 6, 1),
                (9, 7, 0),
                (9, 8, 0),
                (9, 9, 0),
                (9, 10, 0),
                (9, 11, 0),
            ]
        );
        // Sep 6 is 23 hours long; week starts are anchored at the true start of day.
        assert_eq!(
            add_days_start_tz(sep6_sod, 1, Some(&tz)) - sep6_sod,
            23 * HOUR
        );
        let mon_l = to_local_tz(
            start_of_week_with_first_tz(sep6_mid, 0, Some(&tz)),
            Some(&tz),
        );
        assert_eq!(
            (mon_l.year, mon_l.month, mon_l.day, mon_l.hour),
            (2026, 8, 31, 0)
        );
        let sun_l = to_local_tz(
            start_of_week_with_first_tz(sep6_mid, 6, Some(&tz)),
            Some(&tz),
        );
        assert_eq!(
            (sun_l.year, sun_l.month, sun_l.day, sun_l.hour),
            (2026, 9, 6, 1)
        );
    }

    #[test]
    fn month_arithmetic_clamps_short_months() {
        let jan31 = from_local(2026, 1, 31, 9, 0, 0, 0);
        let feb = add_months(jan31, 1);
        let fl = to_local(feb);
        assert_eq!((fl.year, fl.month, fl.day), (2026, 2, 28));
    }
}
