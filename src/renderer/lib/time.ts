export const MIN = 60_000;
export const HOUR = 60 * MIN;
export const DAY = 24 * HOUR;

export const startOfDay = (t: number) => {
  const d = new Date(t);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
};
export const addDays = (t: number, n: number) => {
  const d = new Date(t);
  d.setDate(d.getDate() + n);
  return d.getTime();
};
/** Weeks start on Monday. */
export const startOfWeek = (t: number) => {
  const d = new Date(startOfDay(t));
  const dow = (d.getDay() + 6) % 7;
  d.setDate(d.getDate() - dow);
  return d.getTime();
};
export const startOfMonth = (t: number) => {
  const d = new Date(startOfDay(t));
  d.setDate(1);
  return d.getTime();
};
export const addMonths = (t: number, n: number) => {
  const d = new Date(t);
  d.setMonth(d.getMonth() + n);
  return d.getTime();
};
export const startOfYear = (t: number) => new Date(new Date(t).getFullYear(), 0, 1).getTime();

/** Minutes since local midnight of the day `t` falls in (DST-safe for layout). */
export const minutesOfDay = (t: number) => {
  const d = new Date(t);
  return d.getHours() * 60 + d.getMinutes() + d.getSeconds() / 60;
};
export const atMinutes = (day: number, minutes: number) => {
  const d = new Date(day);
  d.setHours(0, Math.round(minutes), 0, 0);
  return d.getTime();
};

export const sameDay = (a: number, b: number) => startOfDay(a) === startOfDay(b);

/** The length of [start, end) that falls inside [from, to). */
export const overlap = (start: number, end: number, from: number, to: number) => Math.max(0, Math.min(end, to) - Math.max(start, from));

export const isoWeek = (t: number) => {
  const d = new Date(startOfDay(t));
  d.setDate(d.getDate() + 3 - ((d.getDay() + 6) % 7));
  const jan4 = new Date(d.getFullYear(), 0, 4);
  return 1 + Math.round(((d.getTime() - jan4.getTime()) / DAY - 3 + ((jan4.getDay() + 6) % 7)) / 7);
};
