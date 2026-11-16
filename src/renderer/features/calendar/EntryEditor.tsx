import { useEffect, useMemo, useRef, useState } from 'react';
import type { EntrySource, Project } from '@/shared/types';
import { Popover } from '@/renderer/components/Popover';
import { LoButton } from '@/renderer/components/LoButton';
import { LoSelect } from '@/renderer/components/LoSelect';
import { useEntryMutations } from '@/renderer/lib/api';
import { atMinutes, DAY, minutesOfDay, startOfDay } from '@/renderer/lib/time';
import { clockOf, dayLabel, duration, money, parseClock } from '@/renderer/lib/format';
import { ProjectDot } from '@/renderer/components/ProjectDot';

export interface Draft {
  id?: string;
  projectId: string | null;
  description: string;
  start: number;
  end: number | null;
  billable: boolean;
  source: EntrySource;
}

const toDateInput = (t: number) => {
  const d = new Date(t);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
};
const fromDateInput = (s: string) => {
  const [y, m, d] = s.split('-').map(Number);
  return new Date(y, m - 1, d).getTime();
};

/** Create or edit one time entry. Times are typed (09:30, 930, 9) and an end before the start means "the next day". */
export function EntryEditor({
  draft,
  anchor,
  projects,
  onClose,
  closing,
}: {
  draft: Draft;
  anchor: DOMRect | null;
  projects: Project[];
  onClose: () => void;
  closing?: boolean;
}) {
  const m = useEntryMutations();
  const running = draft.end === null;
  const [description, setDescription] = useState(draft.description);
  const [projectId, setProjectId] = useState(draft.projectId ?? '');
  const [billable, setBillable] = useState(draft.billable);
  const [date, setDate] = useState(toDateInput(draft.start));
  const [startText, setStartText] = useState(clockOf(minutesOfDay(draft.start)));
  const [endText, setEndText] = useState(draft.end ? clockOf(minutesOfDay(draft.end)) : '');
  const [error, setError] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);
  // After the pointer-up that opened the editor has settled, or the grid keeps focus.
  useEffect(() => {
    const t = setTimeout(() => inputRef.current?.focus(), 0);
    return () => clearTimeout(t);
  }, []);

  const active = projects.filter((p) => !p.archived || p.id === draft.projectId);
  const project = projects.find((p) => p.id === projectId);

  const times = useMemo(() => {
    const day = fromDateInput(date);
    const s = parseClock(startText);
    if (s === null || !Number.isFinite(day)) return null;
    const start = atMinutes(day, s);
    if (running) return { start, end: null };
    const e = parseClock(endText);
    if (e === null) return null;
    let end = atMinutes(day, e);
    if (end <= start) end = atMinutes(day + DAY, e);
    return { start, end };
  }, [date, startText, endText, running]);

  const length = times ? (times.end ?? Date.now()) - times.start : 0;

  async function save() {
    if (!times) return setError('Use a time like 09:30.');
    if (times.end !== null && times.end - times.start > 24 * 3_600_000) return setError('An entry can be at most 24 hours.');
    const body = { projectId: projectId || null, description, billable, start: times.start, end: times.end, source: draft.source };
    try {
      if (draft.id) await m.update.mutateAsync({ id: draft.id, patch: body });
      else await m.create.mutateAsync(body);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not save the entry.');
    }
  }

  return (
    <Popover anchor={anchor} onClose={onClose} closing={closing} label={draft.id ? 'Edit time entry' : 'New time entry'}>
      <form
        className="fo-editor"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <input
          ref={inputRef}
          className="fo-editor-desc"
          placeholder="What did you work on?"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          spellCheck={false}
        />

        <div className="fo-editor-row">
          <span className="fo-editor-key">Project</span>
          <div className="fo-editor-project">
            <LoSelect
              value={projectId}
              leading={project && <ProjectDot color={project.color} />}
              aria-label="Project"
              placeholder="No project"
              onChange={(e) => {
                setProjectId(e.target.value);
                const p = projects.find((x) => x.id === e.target.value);
                if (p && !draft.id) setBillable(p.billableByDefault);
              }}
            >
              <option value="">No project</option>
              {active.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.client ? `${p.name} · ${p.client}` : p.name}
                </option>
              ))}
            </LoSelect>
          </div>
        </div>

        <div className="fo-editor-row">
          <span className="fo-editor-key">Time</span>
          <div className="fo-editor-times">
            <input className="lo-input fo-time" aria-label="Start" value={startText} onChange={(e) => setStartText(e.target.value)} />
            <span className="fo-dash">–</span>
            {running ? (
              <span className="fo-running-label">now</span>
            ) : (
              <input className="lo-input fo-time" aria-label="End" value={endText} onChange={(e) => setEndText(e.target.value)} />
            )}
            <span className="fo-editor-len">{times ? duration(length) : '—'}</span>
          </div>
        </div>

        <div className="fo-editor-row">
          <span className="fo-editor-key">Date</span>
          <input type="date" className="lo-input fo-date" value={date} onChange={(e) => setDate(e.target.value)} aria-label="Date" />
          <span className="fo-editor-hint">{dayLabel(startOfDay(fromDateInput(date) || draft.start))}</span>
        </div>

        <label className="fo-check">
          <input type="checkbox" checked={billable} onChange={(e) => setBillable(e.target.checked)} />
          <span>Billable</span>
          {billable && project && project.rate > 0 && times && (
            <span className="fo-editor-hint">{money((length / 3_600_000) * project.rate, project.currency)}</span>
          )}
          {billable && (!project || !project.rate) && <span className="fo-editor-hint">no rate on this project</span>}
        </label>

        {error && <div className="fo-error" role="alert">{error}</div>}

        <div className="fo-editor-foot">
          {draft.id && (
            <LoButton
              variant="ghost"
              icon="trash"
              aria-label="Delete entry"
              title="Delete entry"
              onClick={async () => {
                await m.remove.mutateAsync(draft.id!);
                onClose();
              }}
            />
          )}
          <span style={{ flex: 1 }} />
          {running && draft.id && (
            <LoButton
              icon="stop.fill"
              onClick={async () => {
                await save();
                await m.stop.mutateAsync();
              }}
            >
              Stop
            </LoButton>
          )}
          <LoButton onClick={onClose}>Cancel</LoButton>
          <button type="submit" className="lo-btn lo-btn--primary">
            {draft.id ? 'Save' : 'Add entry'}
          </button>
        </div>
      </form>
    </Popover>
  );
}
