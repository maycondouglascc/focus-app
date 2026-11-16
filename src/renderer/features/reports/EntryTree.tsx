import { useMemo, useState, type ReactNode } from 'react';
import type { Project, TimeEntry } from '@/shared/types';
import { ProjectDot } from '@/renderer/components/ProjectDot';
import { NO_PROJECT } from '@/renderer/lib/colors';
import { dayLabel, duration, money, time } from '@/renderer/lib/format';
import { earned, entryMs } from '@/renderer/lib/stats';
import { Icon } from '@/renderer/components/Icon';

const NONE = '__none';

interface Task {
  key: string;
  label: string;
  ms: number;
  earned: number;
  entries: TimeEntry[];
}
interface Group {
  key: string;
  name: string;
  client: string;
  color: string;
  currency: string;
  ms: number;
  earned: number;
  count: number;
  tasks: Task[];
}

function tasksOf(entries: TimeEntry[], project: Project | undefined, from: number, to: number, now: number): Task[] {
  const m = new Map<string, Task>();
  for (const e of entries) {
    const label = e.description.trim() || 'No description';
    const key = label.toLowerCase();
    const t = m.get(key) ?? { key, label, ms: 0, earned: 0, entries: [] };
    t.ms += entryMs(e, from, to, now);
    t.earned += earned(e, project, from, to);
    t.entries.push(e);
    m.set(key, t);
  }
  for (const t of m.values()) t.entries.sort((a, b) => b.start - a.start);
  return [...m.values()].sort((a, b) => b.ms - a.ms);
}

/**
 * Time grouped the way you think about it: project → what you worked on → the individual entries. Every level starts
 * folded. An entry opens the editor (edit, move, delete).
 */
export function EntryTree({
  entries,
  byId,
  from,
  to,
  now,
  byProject,
  onOpen,
}: {
  entries: TimeEntry[];
  byId: Map<string, Project>;
  from: number;
  to: number;
  now: number;
  /** Top level per project; otherwise (one project's report) start at tasks. */
  byProject: boolean;
  onOpen: (e: TimeEntry, anchor: DOMRect) => void;
}) {
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const toggle = (k: string) =>
    setOpen((cur) => {
      const next = new Set(cur);
      if (next.has(k)) next.delete(k);
      else next.add(k);
      return next;
    });

  const groups = useMemo<Group[]>(() => {
    const m = new Map<string, TimeEntry[]>();
    for (const e of entries) {
      const k = byProject ? (e.projectId ?? NONE) : 'all';
      m.set(k, [...(m.get(k) ?? []), e]);
    }
    return [...m.entries()]
      .map(([k, list]) => {
        const p = k === NONE || k === 'all' ? (k === 'all' && list[0]?.projectId ? byId.get(list[0].projectId) : undefined) : byId.get(k);
        const tasks = tasksOf(list, p, from, to, now);
        return {
          key: k,
          name: k === NONE ? 'No project' : (p?.name ?? 'Deleted project'),
          client: p?.client ?? '',
          color: p?.color ?? NO_PROJECT,
          currency: p?.currency ?? '',
          ms: tasks.reduce((s, t) => s + t.ms, 0),
          earned: tasks.reduce((s, t) => s + t.earned, 0),
          count: list.length,
          tasks,
        };
      })
      .sort((a, b) => b.ms - a.ms);
  }, [entries, byId, byProject, from, to, now]);

  const money_ = (v: number, cur: string) => (v > 0 && cur ? money(v, cur) : '');

  const taskList = (g: Group) => (
    <ul className="fo-tree-list">
      {g.tasks.map((t) => {
        const k = `${g.key}/${t.key}`;
        return (
          <li key={k}>
            <Row
              level={byProject ? 2 : 1}
              open={open.has(k)}
              onToggle={() => toggle(k)}
              label={t.label}
              sub={`${t.entries.length} ${t.entries.length === 1 ? 'entry' : 'entries'} · last ${dayLabel(t.entries[0].start)}`}
              ms={t.ms}
              amount={money_(t.earned, g.currency)}
            />
            <Reveal open={open.has(k)}>
              <ul className="fo-tree-entries">
                {t.entries.map((e) => {
                  const v = earned(e, e.projectId ? byId.get(e.projectId) : undefined);
                  return (
                    <li key={e.id}>
                      <button type="button" className="fo-tree-entry" onClick={(ev) => onOpen(e, ev.currentTarget.getBoundingClientRect())}>
                        <span className="fo-tree-date">{dayLabel(e.start)}</span>
                        <span className="fo-tree-time">
                          {time(e.start)} – {e.end ? time(e.end) : 'now'}
                        </span>
                        <span className="fo-tree-num">{duration((e.end ?? now) - e.start)}</span>
                        <span className="fo-tree-num fo-tree-amount">
                          {v ? money(v, g.currency) : e.billable ? '' : <span className="fo-muted">non-billable</span>}
                        </span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            </Reveal>
          </li>
        );
      })}
    </ul>
  );

  if (!groups.length) return <p className="lo-sub">No time entries in this period.</p>;
  if (!byProject) return <div className="fo-tree">{taskList(groups[0])}</div>;

  return (
    <ul className="fo-tree fo-tree-list">
      {groups.map((g) => (
        <li key={g.key}>
          <Row
            level={1}
            open={open.has(g.key)}
            onToggle={() => toggle(g.key)}
            lead={<ProjectDot color={g.color} size={8} />}
            label={g.name}
            sub={[g.client, `${g.tasks.length} ${g.tasks.length === 1 ? 'task' : 'tasks'} · ${g.count} ${g.count === 1 ? 'entry' : 'entries'}`]
              .filter(Boolean)
              .join(' · ')}
            ms={g.ms}
            amount={money_(g.earned, g.currency)}
          />
          <Reveal open={open.has(g.key)}>{taskList(g)}</Reveal>
        </li>
      ))}
    </ul>
  );
}

function Row({
  level,
  open,
  onToggle,
  lead,
  label,
  sub,
  ms,
  amount,
}: {
  level: 1 | 2;
  open: boolean;
  onToggle: () => void;
  lead?: ReactNode;
  label: string;
  sub: string;
  ms: number;
  amount: string;
}) {
  return (
    <button type="button" className="fo-tree-row" data-level={level} data-lead={!!lead} aria-expanded={open} onClick={onToggle}>
      <Icon name="chevron.right" className="fo-tree-chev" aria-hidden="true" />
      {lead}
      <span className="fo-tree-label">
        <span>{label}</span>
        <small>{sub}</small>
      </span>
      <span className="fo-tree-num">{duration(ms)}</span>
      <span className="fo-tree-num fo-tree-amount">{amount}</span>
    </button>
  );
}

function Reveal({ open, children }: { open: boolean; children: ReactNode }) {
  return (
    <div className="lo-reveal" data-open={open} aria-hidden={!open}>
      <div className="lo-reveal-inner">{children}</div>
    </div>
  );
}
