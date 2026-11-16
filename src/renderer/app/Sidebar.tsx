import type { CSSProperties } from 'react';
import { useEntries, useProjects, useRunning } from '@/renderer/lib/api';
import { entryMs } from '@/renderer/lib/stats';
import { addDays, startOfWeek } from '@/renderer/lib/time';
import { hm } from '@/renderer/lib/format';
import { useNow } from '@/renderer/lib/useNow';
import { Icon, type IconName } from '@/renderer/components/Icon';

export type Section = 'calendar' | 'activity' | 'reports' | 'projects' | 'project' | 'settings';

const NAV: { id: Section; label: string; icon: IconName }[] = [
  { id: 'calendar', label: 'Timer', icon: 'calendar' },
  { id: 'activity', label: 'Activity', icon: 'waveform.path.ecg' },
  { id: 'reports', label: 'Reports', icon: 'chart.bar' },
  { id: 'projects', label: 'Projects', icon: 'folder' },
];

const DRAG = { WebkitAppRegion: 'drag' } as CSSProperties;

export function Sidebar({
  section,
  projectId,
  onSelect,
  onProject,
}: {
  section: Section;
  projectId: string | null;
  onSelect: (s: Section) => void;
  onProject: (id: string) => void;
}) {
  const now = useNow(30_000);
  const from = startOfWeek(now);
  const week = useEntries({ from, to: addDays(from, 7) });
  const projects = useProjects();
  const running = useRunning();
  const active = (projects.data ?? []).filter((p) => !p.archived);
  const totals = new Map<string, number>();
  for (const e of week.data ?? []) totals.set(e.projectId ?? '', (totals.get(e.projectId ?? '') ?? 0) + entryMs(e, from, Infinity, now));

  return (
    <nav aria-label="Primary" className="shell-sidebar">
      <div className="shell-sidebar-drag" style={DRAG}>
        <span className="shell-brand">Focus</span>
      </div>

      <div className="fo-nav">
        {NAV.map((n) => (
          <button
            key={n.id}
            type="button"
            className="shell-row fo-navrow"
            data-selected={section === n.id}
            aria-current={section === n.id ? 'page' : undefined}
            onClick={() => onSelect(n.id)}
          >
            <Icon name={n.icon} />
            <span>{n.label}</span>
            {n.id === 'calendar' && running.data && <span className="fo-live-dot" aria-label="Timer running" />}
          </button>
        ))}
      </div>

      <div className="shell-team-head">
        <span className="fo-side-label">This week</span>
      </div>
      <div className="shell-team" role="list">
        {projects.isSuccess && active.length === 0 && (
          <button type="button" className="fo-side-empty" onClick={() => onSelect('projects')}>
            <Icon name="plus" size={14} />
            Create a project
          </button>
        )}
        {active.map((p) => (
          <div role="listitem" key={p.id}>
            <button
              type="button"
              className="shell-row shell-person fo-navrow fo-side-project"
              data-selected={section === 'project' && projectId === p.id}
              aria-current={section === 'project' && projectId === p.id ? 'page' : undefined}
              onClick={() => onProject(p.id)}
            >
              <span className="shell-dot" style={{ background: p.color }} aria-hidden="true" />
              <span className="shell-person-name">{p.name}</span>
              <span className="fo-side-total">{totals.get(p.id) ? hm(totals.get(p.id)!) : ''}</span>
            </button>
          </div>
        ))}
      </div>

      <button
        type="button"
        className="shell-row fo-navrow shell-settings"
        data-selected={section === 'settings'}
        onClick={() => onSelect('settings')}
      >
        <Icon name="gearshape" aria-hidden="true" />
        <span>Settings</span>
      </button>
    </nav>
  );
}
