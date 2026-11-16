import { useMemo, useState } from 'react';
import type { Project } from '@/shared/types';
import { LoButton } from '@/renderer/components/LoButton';
import { LoDialog } from '@/renderer/components/LoDialog';
import { LoField, LoInput } from '@/renderer/components/LoField';
import { LoSelect } from '@/renderer/components/LoSelect';
import { ProjectDot } from '@/renderer/components/ProjectDot';
import { useEntries, useProjectMutations, useProjects, useSettings } from '@/renderer/lib/api';
import { PROJECT_COLORS } from '@/renderer/lib/colors';
import { hm, money } from '@/renderer/lib/format';
import { earned, entryMs } from '@/renderer/lib/stats';
import { startOfMonth } from '@/renderer/lib/time';
import { Icon } from '@/renderer/components/Icon';

const CURRENCIES = ['PLN', 'EUR', 'USD', 'GBP', 'CHF', 'SEK', 'NOK', 'DKK', 'CZK'];

export function ProjectsScreen({ onOpen }: { onOpen: (id: string) => void }) {
  const projects = useProjects();
  const all = useEntries({ from: 0, to: 8_640_000_000_000 }).data ?? [];
  const [editing, setEditing] = useState<Project | 'new' | null>(null);
  const [showArchived, setShowArchived] = useState(false);
  const monthFrom = startOfMonth(Date.now());

  const stats = useMemo(() => {
    const m = new Map<string, { total: number; month: number; earned: number }>();
    const byId = new Map((projects.data ?? []).map((p) => [p.id, p]));
    for (const e of all) {
      if (!e.projectId) continue;
      const s = m.get(e.projectId) ?? { total: 0, month: 0, earned: 0 };
      s.total += entryMs(e);
      s.month += entryMs(e, monthFrom);
      s.earned += earned(e, byId.get(e.projectId));
      m.set(e.projectId, s);
    }
    return m;
  }, [all, projects.data, monthFrom]);

  const list = (projects.data ?? []).filter((p) => showArchived || !p.archived);
  const archivedCount = (projects.data ?? []).filter((p) => p.archived).length;

  return (
    <div className="page fo-page">
      <div className="page-inner">
        <div className="fo-pagehead">
          <div>
            <h1 className="fo-headline">{list.length ? `${list.length} ${list.length === 1 ? 'project' : 'projects'}` : 'Projects'}</h1>
          </div>
          <LoButton variant="primary" icon="plus" onClick={() => setEditing('new')}>
            New project
          </LoButton>
        </div>

        {projects.isSuccess && list.length === 0 ? (
          <div className="lo-card fo-empty">
            <h2 className="lo-h2">Start with a project</h2>
            <p className="lo-sub">A project holds a billing rate. Time you log on it adds up to hours and earnings in Reports.</p>
            <div>
              <LoButton variant="primary" icon="plus" onClick={() => setEditing('new')}>
                New project
              </LoButton>
            </div>
          </div>
        ) : (
          <div className="lo-card">
            <table className="fo-table fo-table--projects">
              <thead>
                <tr>
                  <th>Project</th>
                  <th className="num">Rate</th>
                  <th className="num">This month</th>
                  <th className="num">All time</th>
                  <th className="num">Earned</th>
                </tr>
              </thead>
              <tbody>
                {list.map((p) => {
                  const s = stats.get(p.id);
                  return (
                    <tr key={p.id} className="fo-row-click" onClick={() => onOpen(p.id)} data-archived={p.archived}>
                      <td>
                        <span className="fo-cell-project">
                          <ProjectDot color={p.color} size={8} />
                          <span>
                            {p.name}
                            {(p.client || p.archived) && <small>{[p.client, p.archived && 'Archived'].filter(Boolean).join(' · ')}</small>}
                          </span>
                        </span>
                      </td>
                      <td className="num">{p.rate ? `${money(p.rate, p.currency)}/h` : <span className="fo-muted">internal</span>}</td>
                      <td className="num">{s?.month ? hm(s.month) : '—'}</td>
                      <td className="num">{s?.total ? hm(s.total) : '—'}</td>
                      <td className="num">{s?.earned ? money(s.earned, p.currency) : '—'}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
        {archivedCount > 0 && (
          <button type="button" className="lo-link fo-archived-toggle" onClick={() => setShowArchived(!showArchived)}>
            {showArchived ? 'Hide archived' : `Show ${archivedCount} archived`}
          </button>
        )}
      </div>

      {editing && <ProjectDialog project={editing === 'new' ? null : editing} usedColors={(projects.data ?? []).map((p) => p.color)} onClose={() => setEditing(null)} />}
    </div>
  );
}

export function ProjectDialog({ project, usedColors, onClose }: { project: Project | null; usedColors: string[]; onClose: () => void }) {
  const settings = useSettings().data;
  const m = useProjectMutations();
  const [name, setName] = useState(project?.name ?? '');
  const [client, setClient] = useState(project?.client ?? '');
  const [color, setColor] = useState(project?.color ?? PROJECT_COLORS.find((c) => !usedColors.includes(c)) ?? PROJECT_COLORS[0]);
  const [rate, setRate] = useState(project?.rate ? String(project.rate) : '');
  const [currency, setCurrency] = useState(project?.currency ?? settings?.currency ?? 'PLN');
  const [billable, setBillable] = useState(project?.billableByDefault ?? true);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState('');

  const rateNum = rate.trim() === '' ? 0 : Number(rate.replace(',', '.'));
  const valid = name.trim().length > 0 && Number.isFinite(rateNum) && rateNum >= 0;

  async function save() {
    if (!valid) return setError(!name.trim() ? 'Give the project a name.' : 'The rate must be a number, like 150.');
    const body = { name, client, color, rate: rateNum, currency, billableByDefault: billable };
    try {
      if (project) await m.update.mutateAsync({ id: project.id, patch: body });
      else await m.create.mutateAsync(body);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not save the project.');
    }
  }

  return (
    <LoDialog
      open
      onClose={onClose}
      title={project ? 'Edit project' : 'New project'}
      description={project ? undefined : 'Time logged on this project is valued at its hourly rate.'}
      footer={
        confirmDelete ? (
          <>
            <span className="fo-confirm-text">Delete “{project?.name}”? Its time entries stay, without a project.</span>
            <LoButton onClick={() => setConfirmDelete(false)}>Keep</LoButton>
            <LoButton
              variant="danger"
              onClick={async () => {
                await m.remove.mutateAsync(project!.id);
                onClose();
              }}
            >
              Delete project
            </LoButton>
          </>
        ) : (
          <>
            {project && (
              <>
                <LoButton variant="ghost" onClick={() => setConfirmDelete(true)}>
                  Delete
                </LoButton>
                <LoButton
                  variant="ghost"
                  onClick={async () => {
                    await m.update.mutateAsync({ id: project.id, patch: { archived: !project.archived } });
                    onClose();
                  }}
                >
                  {project.archived ? 'Unarchive' : 'Archive'}
                </LoButton>
              </>
            )}
            <span style={{ flex: 1 }} />
            <LoButton onClick={onClose}>Cancel</LoButton>
            <LoButton variant="primary" onClick={save}>
              {project ? 'Save' : 'Create project'}
            </LoButton>
          </>
        )
      }
    >
      <form
        className="fo-form"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <LoField label="Name" required>
          {(id) => <LoInput id={id} autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="Website redesign" />}
        </LoField>
        <LoField label="Client">
          {(id) => <LoInput id={id} value={client} onChange={(e) => setClient(e.target.value)} placeholder="Optional" />}
        </LoField>
        <div className="fo-form-row">
          <LoField label="Hourly rate">
            {(id) => <LoInput id={id} inputMode="decimal" value={rate} onChange={(e) => setRate(e.target.value)} placeholder="0 = internal" />}
          </LoField>
          <LoField label="Currency">
            {(id) => (
              <LoSelect id={id} value={currency} onChange={(e) => setCurrency(e.target.value)}>
                {CURRENCIES.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </LoSelect>
            )}
          </LoField>
        </div>
        <div className="lo-field">
          <span className="lo-label">Colour</span>
          <div className="fo-swatches" role="radiogroup" aria-label="Colour">
            {PROJECT_COLORS.map((c) => (
              <button key={c} type="button" role="radio" aria-checked={c === color} aria-label={c} className="fo-swatch" style={{ background: c }} onClick={() => setColor(c)}>
                {c === color && <Icon name="checkmark" aria-hidden="true" />}
              </button>
            ))}
          </div>
        </div>
        <label className="fo-check">
          <input type="checkbox" checked={billable} onChange={(e) => setBillable(e.target.checked)} />
          <span>New entries are billable</span>
        </label>
        {error && <div className="fo-error">{error}</div>}
        <button type="submit" hidden />
      </form>
    </LoDialog>
  );
}
