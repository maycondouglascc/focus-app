import { useCallback, useMemo, useState } from 'react';
import { EntryTree } from './EntryTree';
import { EntryEditor, type Draft } from '@/renderer/features/calendar/EntryEditor';
import { usePresence } from '@/renderer/lib/usePresence';
import type { Project, TimeEntry } from '@/shared/types';
import { firstStart, fitPeriod, PeriodNav, periodTitle, usePeriod } from '@/renderer/components/PeriodNav';
import { ProportionBar, StackedBars, type Bucket } from '@/renderer/components/charts';
import { Segmented } from '@/renderer/components/Segmented';
import { LoButton } from '@/renderer/components/LoButton';
import { LoSelect } from '@/renderer/components/LoSelect';
import { useActivity, useEntries, useProjects, useSettings } from '@/renderer/lib/api';
import { NO_PROJECT } from '@/renderer/lib/colors';
import { duration, money, time } from '@/renderer/lib/format';
import { earned, earningsByCurrency, entryMs, unionMs } from '@/renderer/lib/stats';
import { HOUR, startOfDay } from '@/renderer/lib/time';

const NONE = '__none';

function csv(entries: TimeEntry[], byId: Map<string, Project>) {
  const q = (s: string) => `"${s.replace(/"/g, '""')}"`;
  const rows = [['Date', 'Start', 'End', 'Hours', 'Project', 'Client', 'Description', 'Billable', 'Rate', 'Currency', 'Amount']];
  for (const e of entries) {
    const p = e.projectId ? byId.get(e.projectId) : undefined;
    const end = e.end ?? Date.now();
    const d = new Date(e.start);
    rows.push([
      `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`,
      time(e.start),
      time(end),
      ((end - e.start) / HOUR).toFixed(2),
      p?.name ?? '',
      p?.client ?? '',
      e.description,
      e.billable ? 'yes' : 'no',
      p ? String(p.rate) : '',
      p?.currency ?? '',
      earned(e, p).toFixed(2),
    ]);
  }
  return rows.map((r) => r.map(q).join(',')).join('\n');
}

export function ReportsScreen() {
  const ctl = usePeriod('month');
  const range = { from: ctl.period.from, to: ctl.period.to };
  const projects = useProjects().data ?? [];
  const settings = useSettings().data;
  const all = useEntries(range).data ?? [];
  const activity = useActivity(range).data ?? [];
  const period = useMemo(() => fitPeriod(ctl.period, firstStart(all)), [ctl.period, all]);
  const [projectFilter, setProjectFilter] = useState('');
  const [metric, setMetric] = useState<'hours' | 'earned'>('hours');
  const byId = useMemo(() => new Map(projects.map((p) => [p.id, p])), [projects]);
  const now = Date.now();

  const entries = projectFilter ? all.filter((e) => (e.projectId ?? NONE) === projectFilter) : all;
  const ms = (e: TimeEntry, from = period.from, to = period.to) => entryMs(e, from, to, now);
  const total = entries.reduce((s, e) => s + ms(e), 0);
  const billable = entries.filter((e) => e.billable).reduce((s, e) => s + ms(e), 0);
  const money_ = earningsByCurrency(entries, byId, period.from, period.to);
  const days = new Set(entries.map((e) => startOfDay(e.start))).size;
  const active = unionMs(activity.filter((a) => a.kind === 'app'), period.from, period.to);

  // The earnings chart uses one currency: the default if it earned anything, otherwise the biggest earner.
  const chartCurrency = money_.find(([c]) => c === settings?.currency)?.[0] ?? money_[0]?.[0] ?? settings?.currency ?? 'USD';
  const mixed = money_.length > 1;

  // Per project.
  const perProject = new Map<string, { ms: number; billMs: number; earned: number }>();
  for (const e of entries) {
    const k = e.projectId ?? NONE;
    const cur = perProject.get(k) ?? { ms: 0, billMs: 0, earned: 0 };
    cur.ms += ms(e);
    if (e.billable) cur.billMs += ms(e);
    cur.earned += earned(e, e.projectId ? byId.get(e.projectId) : undefined, period.from, period.to);
    perProject.set(k, cur);
  }
  const projRows = [...perProject.entries()].sort((a, b) => b[1].ms - a[1].ms);
  const meta = (k: string) => (k === NONE ? { name: 'No project', color: NO_PROJECT, client: '', rate: 0, currency: '' } : byId.get(k) ?? { name: 'Deleted project', color: NO_PROJECT, client: '', rate: 0, currency: '' });

  const buckets: Bucket[] = period.buckets.map((b) => ({
    key: String(b.from),
    label: b.label,
    title: b.title,
    segs: projRows.map(([k]) => {
      const m = meta(k);
      const inBucket = entries.filter((e) => (e.projectId ?? NONE) === k);
      const value =
        metric === 'hours'
          ? inBucket.reduce((s, e) => s + ms(e, b.from, b.to), 0)
          : inBucket.reduce((s, e) => {
              const p = e.projectId ? byId.get(e.projectId) : undefined;
              return p?.currency === chartCurrency ? s + earned(e, p, b.from, b.to) : s;
            }, 0);
      return { key: k, label: m.name, color: m.color, value };
    }),
  }));

  const download = () => {
    const blob = new Blob([csv(entries, byId)], { type: 'text/csv' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = `focus-${periodTitle(period).replace(/[^\w]+/g, '-').toLowerCase()}.csv`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  };

  const [editing, setEditing] = useState<{ draft: Draft; anchor: DOMRect } | null>(null);
  const [shown, closing] = usePresence(editing);
  const openEntry = (e: TimeEntry, anchor: DOMRect) => setEditing(editing?.draft.id === e.id ? null : { draft: { ...e }, anchor });
  const closeEditor = useCallback(() => setEditing(null), []);

  return (
    <div className="page fo-page">
      <div className="page-inner page-inner--wide">
        <div className="fo-pagehead">
          {/* The title is the report's subject: which projects it covers. */}
          <div className="fo-titleselect">
            <LoSelect value={projectFilter} onChange={(e) => setProjectFilter(e.target.value)} aria-label="Project filter" placeholder="All projects">
              <option value="">All projects</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
              <option value={NONE}>No project</option>
            </LoSelect>
          </div>
          <PeriodNav ctl={ctl}>
            <LoButton icon="square.and.arrow.down" onClick={download} disabled={!entries.length} aria-label="Export CSV" title="Export CSV" />
          </PeriodNav>
        </div>

        <div className="lo-stats fo-stats">
          <div className="lo-stat" data-first="true">
            <div className="lo-meta">Tracked · {periodTitle(period)}</div>
            <div className="lo-stat-value">{duration(total)}</div>
            <div className="lo-stat-lines">
              <span>{days ? `${duration(total / days)} per working day` : 'No days tracked'}</span>
            </div>
          </div>
          <div className="lo-stat">
            <div className="lo-meta">Billable</div>
            <div className="lo-stat-value">{duration(billable)}</div>
            <div className="lo-stat-lines">
              <span>{total ? `${Math.round((billable / total) * 100)}% of tracked time` : '—'}</span>
            </div>
          </div>
          <div className="lo-stat">
            <div className="lo-meta">Earned</div>
            <div className="lo-stat-value">{money_.length ? money(money_[0][1], money_[0][0]) : '—'}</div>
            <div className="lo-stat-lines">
              {money_.slice(1).map(([c, v]) => (
                <span key={c}>+ {money(v, c)}</span>
              ))}
              {!money_.length && <span>Set a rate on a project to see earnings</span>}
              {money_.length === 1 && billable > 0 && <span>{money(money_[0][1] / (billable / HOUR), money_[0][0])} per billable hour</span>}
            </div>
          </div>
          <div className="lo-stat">
            <div className="lo-meta">Active on the computer</div>
            <div className="lo-stat-value">{duration(active)}</div>
            <div className="lo-stat-lines">
              <span>{active ? `${Math.round((total / active) * 100)}% of it logged as time` : 'No activity recorded'}</span>
            </div>
          </div>
        </div>

        <section className="lo-card fo-card">
          <div className="fo-card-head">
            <h2 className="lo-h2">
              {metric === 'hours' ? 'Hours' : 'Earnings'} by {period.unit}
              {metric === 'earned' && mixed && <small>in {chartCurrency} only</small>}
            </h2>
            <Segmented
              label="Chart metric"
              value={metric}
              onChange={setMetric}
              options={[
                { value: 'hours', label: 'Hours' },
                { value: 'earned', label: 'Earnings' },
              ]}
            />
          </div>
          <StackedBars
            buckets={buckets}
            format={metric === 'hours' ? duration : (v) => money(v, chartCurrency)}
            axisFormat={metric === 'hours' ? (v) => `${Math.round(v / HOUR)}h` : (v) => money(v, chartCurrency)}
            unit={metric === 'hours' ? HOUR : 10}
            height={240}
            emptyLabel={metric === 'hours' ? 'Nothing tracked in this period.' : 'No billable time with a rate in this period.'}
          />
        </section>

        <section className="lo-card fo-card">
          <h2 className="lo-h2">Projects and time entries</h2>
          {projRows.length === 0 ? (
            <p className="lo-sub">No time entries in this period.</p>
          ) : (
            <>
              <ProportionBar segs={projRows.map(([k, v]) => ({ key: k, label: meta(k).name, color: meta(k).color, value: v.ms }))} format={duration} />
              <EntryTree entries={entries} byId={byId} from={period.from} to={period.to} now={now} byProject onOpen={openEntry} />
            </>
          )}
        </section>
      </div>
      {shown && (
        <EntryEditor
          key={shown.draft.id}
          draft={shown.draft}
          anchor={shown.anchor}
          projects={projects}
          onClose={closeEditor}
          closing={closing}
        />
      )}
    </div>
  );
}
