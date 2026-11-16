import { HOUR, MIN } from './time';

/** 1h 30m · 45m · 0m */
export function duration(ms: number) {
  const m = Math.round(ms / MIN);
  const h = Math.floor(m / 60);
  const r = m % 60;
  if (!h) return `${r}m`;
  // Past 100 hours the minutes are noise (all-time totals): whole hours, grouped — "5,228h".
  if (h >= 100) return `${Math.round(ms / (60 * MIN)).toLocaleString()}h`;
  return r ? `${h}h ${r}m` : `${h}h`;
}

/** 1:30 — compact hours:minutes for tight spots and totals. */
export function hm(ms: number) {
  const m = Math.round(ms / MIN);
  return `${Math.floor(m / 60)}:${String(m % 60).padStart(2, '0')}`;
}

/** 1:05:09 — the running timer. */
export function clock(ms: number) {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 3600)}:${String(Math.floor((s % 3600) / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}

export const hours = (ms: number, digits = 1) => `${(ms / HOUR).toFixed(digits).replace(/\.0+$/, '')}h`;

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });
export const time = (t: number) => timeFmt.format(t);

const dayFmt = new Intl.DateTimeFormat(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
export const dayLabel = (t: number) => dayFmt.format(t);

const shortDate = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short' });
export const shortDay = (t: number) => shortDate.format(t);

export function rangeLabel(from: number, toExclusive: number) {
  const a = new Date(from);
  const b = new Date(toExclusive - 1);
  const sameYear = a.getFullYear() === b.getFullYear();
  const sameMonth = sameYear && a.getMonth() === b.getMonth();
  const month = (d: Date) => d.toLocaleString(undefined, { month: 'short' });
  if (sameMonth && a.getDate() === b.getDate()) return `${month(a)} ${a.getDate()}, ${a.getFullYear()}`;
  if (sameMonth) return `${month(a)} ${a.getDate()} – ${b.getDate()}, ${a.getFullYear()}`;
  if (sameYear) return `${month(a)} ${a.getDate()} – ${month(b)} ${b.getDate()}, ${a.getFullYear()}`;
  return `${month(a)} ${a.getDate()}, ${a.getFullYear()} – ${month(b)} ${b.getDate()}, ${b.getFullYear()}`;
}

const moneyFmts = new Map<string, Intl.NumberFormat>();
export function money(amount: number, currency: string) {
  const key = currency;
  let f = moneyFmts.get(key);
  if (!f) {
    try {
      f = new Intl.NumberFormat(undefined, { style: 'currency', currency, maximumFractionDigits: 0 });
    } catch {
      f = new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 });
    }
    moneyFmts.set(key, f);
  }
  return f.format(amount);
}

/** "09:30" ⇄ minutes. Accepts 9, 930, 9:30, 9.30, 21:05. */
export function parseClock(s: string): number | null {
  const t = s.trim().replace('.', ':');
  let h: number, m: number;
  if (/^\d{1,2}$/.test(t)) [h, m] = [Number(t), 0];
  else if (/^\d{3,4}$/.test(t)) [h, m] = [Number(t.slice(0, -2)), Number(t.slice(-2))];
  else if (/^\d{1,2}:\d{1,2}$/.test(t)) [h, m] = t.split(':').map(Number) as [number, number];
  else return null;
  if (h > 24 || m > 59 || (h === 24 && m > 0)) return null;
  return h * 60 + m;
}
export const clockOf = (minutes: number) =>
  `${String(Math.floor(minutes / 60)).padStart(2, '0')}:${String(Math.round(minutes % 60)).padStart(2, '0')}`;
