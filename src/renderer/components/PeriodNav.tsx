import { LoSelect } from './LoSelect';
import { useMemo, useState, type ReactNode } from 'react';
import { addDays, addMonths, startOfDay, startOfMonth, startOfWeek, startOfYear, isoWeek, HOUR } from '@/renderer/lib/time';
import { rangeLabel } from '@/renderer/lib/format';
import { Icon } from '@/renderer/components/Icon';

export type PeriodKind = 'day' | 'week' | 'month' | 'year' | 'all';

export interface Period {
  kind: PeriodKind;
  from: number;
  to: number;
  /** What one chart bar is. */
  unit: 'hour' | 'day' | 'month' | 'year';
  /** Chart buckets: hours of a day, days of a week/month, months of a year, months or years of all time. */
  buckets: { from: number; to: number; label: string; title: string }[];
}

const monthBucket = (a: number, label: string) => ({
  from: a,
  to: addMonths(a, 1),
  label,
  title: new Date(a).toLocaleDateString(undefined, { month: 'long', year: 'numeric' }),
});

/**
 * All time, before the data is known: the query range (everything up to the end of this month). `fitPeriod` turns it
 * into real bars once the screen knows where its data starts.
 */
function build(kind: PeriodKind, anchor: number): Period {
  if (kind === 'all') return { kind, from: 0, to: addMonths(startOfMonth(Date.now()), 1), unit: 'month', buckets: [] };
  if (kind === 'day') {
    const from = startOfDay(anchor);
    const to = addDays(from, 1);
    const buckets = Array.from({ length: 24 }, (_, h) => ({
      from: from + h * HOUR,
      to: from + (h + 1) * HOUR,
      label: h % 3 === 0 ? String(h).padStart(2, '0') : '',
      title: `${String(h).padStart(2, '0')}:00 – ${String(h + 1).padStart(2, '0')}:00`,
    }));
    return { kind, from, to, unit: 'hour', buckets };
  }
  if (kind === 'year') {
    const from = startOfYear(anchor);
    const buckets = Array.from({ length: 12 }, (_, i) => {
      const a = addMonths(from, i);
      return monthBucket(a, new Date(a).toLocaleDateString(undefined, { month: 'short' }));
    });
    return { kind, from, to: addMonths(from, 12), unit: 'month', buckets };
  }
  const from = kind === 'week' ? startOfWeek(anchor) : startOfMonth(anchor);
  const to = kind === 'week' ? addDays(from, 7) : addMonths(from, 1);
  const buckets: Period['buckets'] = [];
  for (let d = from; d < to; d = addDays(d, 1)) {
    const date = new Date(d);
    buckets.push({
      from: d,
      to: addDays(d, 1),
      label: kind === 'week' ? date.toLocaleDateString(undefined, { weekday: 'short' }) : String(date.getDate()),
      title: date.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'short' }),
    });
  }
  return { kind, from, to, unit: 'day', buckets };
}

/**
 * All time from where the data starts: one bar per month (labels thin out to quarters, then to years),
 * or one per year past four years. Other periods pass through.
 */
export function fitPeriod(p: Period, earliest: number | undefined): Period {
  if (p.kind !== 'all') return p;
  const end = p.to;
  let from = startOfMonth(Math.min(earliest ?? Date.now(), Date.now()));
  let months = 0;
  for (let m = from; m < end; m = addMonths(m, 1)) months++;
  if (months > 48) {
    from = startOfYear(from);
    const buckets: Period['buckets'] = [];
    for (let y = from; y < end; y = addMonths(y, 12)) {
      const year = String(new Date(y).getFullYear());
      buckets.push({ from: y, to: addMonths(y, 12), label: year, title: year });
    }
    return { ...p, from, unit: 'year', buckets };
  }
  // Up to 16 bars every month is labelled, up to 24 every quarter, beyond that only each January, as the year.
  const quarters = months > 16 && months <= 24;
  const years = months > 24;
  if (quarters) from = addMonths(from, -(new Date(from).getMonth() % 3)); // start on a quarter so labels line up
  const buckets: Period['buckets'] = [];
  for (let m = from; m < end; m = addMonths(m, 1)) {
    const d = new Date(m);
    const jan = d.getMonth() === 0;
    const label = years
      ? jan ? String(d.getFullYear()) : ''
      : quarters && d.getMonth() % 3 !== 0
        ? ''
        : jan || m === from
          ? d.toLocaleDateString(undefined, { month: 'short', year: '2-digit' })
          : d.toLocaleDateString(undefined, { month: 'short' });
    buckets.push(monthBucket(m, label));
  }
  return { ...p, from, unit: 'month', buckets };
}

/** Where a screen's data starts, for `fitPeriod`. */
export const firstStart = (xs: { start: number }[]) => (xs.length ? xs.reduce((m, x) => Math.min(m, x.start), Infinity) : undefined);

export function usePeriod(initial: PeriodKind = 'week') {
  const [kind, setKind] = useState<PeriodKind>(initial);
  const [anchor, setAnchor] = useState(() => Date.now());
  const period = useMemo(() => build(kind, anchor), [kind, anchor]);
  const step = (dir: -1 | 1) =>
    kind !== 'all' &&
    setAnchor((a) => (kind === 'day' ? addDays(a, dir) : kind === 'week' ? addDays(a, 7 * dir) : kind === 'month' ? addMonths(startOfMonth(a), dir) : addMonths(startOfYear(a), 12 * dir)));
  return { period, kind, setKind, step, reset: () => setAnchor(Date.now()) };
}

export function periodTitle(p: Period) {
  if (p.kind === 'all') return 'All time';
  const now = Date.now();
  const current = now >= p.from && now < p.to;
  if (p.kind === 'day') return current ? 'Today' : rangeLabel(p.from, p.to);
  if (p.kind === 'week') return current ? `This week · W${isoWeek(p.from)}` : `${rangeLabel(p.from, p.to)} · W${isoWeek(p.from)}`;
  if (p.kind === 'month') return new Date(p.from).toLocaleDateString(undefined, { month: 'long', year: 'numeric' });
  return String(new Date(p.from).getFullYear());
}

const KINDS: { value: PeriodKind; label: string }[] = [
  { value: 'day', label: 'Day' },
  { value: 'week', label: 'Week' },
  { value: 'month', label: 'Month' },
  { value: 'year', label: 'Year' },
  { value: 'all', label: 'All time' },
];

/**
 * ‹ period › as one control with a fixed-width title, plus the period length in a dropdown. Nothing in it changes
 * width when you step or switch, so the page under it never moves. Clicking the title goes back to now.
 */
export function PeriodNav({ ctl, children }: { ctl: ReturnType<typeof usePeriod>; children?: ReactNode }) {
  const now = Date.now();
  const all = ctl.kind === 'all';
  const current = all || (now >= ctl.period.from && now < ctl.period.to);
  return (
    <div className="fo-period">
      <div className="fo-period-step" role="group" aria-label="Period">
        <button type="button" className="fo-period-arrow" aria-label="Previous period" onClick={() => ctl.step(-1)} disabled={all}>
          <Icon name="chevron.left" aria-hidden="true" />
        </button>
        <button
          type="button"
          className="fo-period-title"
          onClick={ctl.reset}
          disabled={current}
          title={current ? undefined : 'Back to now'}
        >
          {periodTitle(ctl.period)}
        </button>
        <button type="button" className="fo-period-arrow" aria-label="Next period" onClick={() => ctl.step(1)} disabled={all}>
          <Icon name="chevron.right" aria-hidden="true" />
        </button>
      </div>
      <div className="fo-period-kind">
        <LoSelect value={ctl.kind} onChange={(e) => ctl.setKind(e.target.value as PeriodKind)} aria-label="Period length">
          {KINDS.map((k) => (
            <option key={k.value} value={k.value}>
              {k.label}
            </option>
          ))}
        </LoSelect>
      </div>
      {children}
    </div>
  );
}
