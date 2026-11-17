//! Pure aggregation helpers for time entries and activity intervals,
//! matching Electron `src/renderer/lib/stats.ts`.
#![allow(dead_code)] // wired across F3 screens in subsequent commits

use std::collections::HashMap;

use crate::db::{Project, TimeEntry};
use crate::time::{overlap, HOUR};

pub fn entry_end(e: &TimeEntry, now: i64) -> i64 {
    e.end.unwrap_or(now)
}

pub fn entry_ms(e: &TimeEntry, from: i64, to: i64, now: i64) -> i64 {
    overlap(e.start, entry_end(e, now), from, to)
}

pub fn earned(e: &TimeEntry, project: Option<&Project>, from: i64, to: i64, now: i64) -> f64 {
    let Some(p) = project else {
        return 0.0;
    };
    if !e.billable || p.rate <= 0.0 {
        return 0.0;
    }
    (entry_ms(e, from, to, now) as f64 / HOUR as f64) * p.rate
}

/// Sums earnings grouped by currency (`PLN` and `EUR` never combine into one number).
pub fn earnings_by_currency(
    entries: &[TimeEntry],
    by_id: &HashMap<&str, &Project>,
    from: i64,
    to: i64,
    now: i64,
) -> Vec<(String, f64)> {
    let mut out: HashMap<String, f64> = HashMap::new();
    for e in entries {
        let Some(pid) = e.project_id.as_deref() else {
            continue;
        };
        let Some(&p) = by_id.get(pid) else {
            continue;
        };
        let v = earned(e, Some(p), from, to, now);
        if v > 0.0 {
            *out.entry(p.currency.clone()).or_insert(0.0) += v;
        }
    }
    let mut list: Vec<_> = out.into_iter().collect();
    list.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    list
}

/// Wall-clock duration covered by the union of `(start, end)` intervals clipped to `[from, to)`.
pub fn union_ms(items: &[(i64, i64)], from: i64, to: i64) -> i64 {
    let mut iv: Vec<(i64, i64)> = items
        .iter()
        .filter_map(|&(s, e)| {
            let a = s.max(from);
            let b = e.min(to);
            (b > a).then_some((a, b))
        })
        .collect();
    iv.sort_unstable_by_key(|x| (x.0, x.1));
    union_ms_sorted(&iv)
}

/// Fast O(1)-space interval union when `intervals` are already sorted by `start` and clipped to `[from, to)`.
pub fn union_ms_sorted(sorted: &[(i64, i64)]) -> i64 {
    let mut total = 0i64;
    let mut cur: Option<(i64, i64)> = None;
    for &(a, b) in sorted {
        if b <= a {
            continue;
        }
        match cur {
            Some((cs, ce)) if a <= ce => cur = Some((cs, ce.max(b))),
            Some((cs, ce)) => {
                total += ce - cs;
                cur = Some((a, b));
            }
            None => cur = Some((a, b)),
        }
    }
    if let Some((cs, ce)) = cur {
        total += ce - cs;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::MIN;

    fn proj(id: &str, rate: f64, currency: &str) -> Project {
        Project {
            id: id.into(),
            name: id.into(),
            client: String::new(),
            color: "#3b7ddd".into(),
            rate,
            currency: currency.into(),
            billable_default: true,
            archived: false,
            created_at: 1,
        }
    }

    fn entry(pid: Option<&str>, start: i64, end: Option<i64>, billable: bool) -> TimeEntry {
        TimeEntry {
            id: "e".into(),
            project_id: pid.map(str::to_string),
            description: String::new(),
            start,
            end,
            billable,
            source: "manual".into(),
        }
    }

    #[test]
    fn earnings_never_mix_currencies_and_respect_billable_flag() {
        let p_usd = proj("p1", 100.0, "USD");
        let p_brl = proj("p2", 200.0, "BRL");
        let mut by_id = HashMap::new();
        by_id.insert("p1", &p_usd);
        by_id.insert("p2", &p_brl);

        let entries = vec![
            entry(Some("p1"), 0, Some(90 * MIN), true),  // $150
            entry(Some("p1"), 0, Some(60 * MIN), false), // non-billable -> $0
            entry(Some("p2"), 0, Some(60 * MIN), true),  // R$ 200
        ];
        let res = earnings_by_currency(&entries, &by_id, 0, HOUR * 10, HOUR * 10);
        assert_eq!(res, vec![("BRL".into(), 200.0), ("USD".into(), 150.0)]);
    }

    #[test]
    fn union_ms_counts_overlapping_intervals_once() {
        let items = vec![(10, 50), (30, 80), (100, 120)];
        assert_eq!(union_ms(&items, 0, 200), 90);
        assert_eq!(union_ms(&items, 20, 110), 70);
    }
}
