import { useEffect, useState } from 'react';
import type { Project } from '@/shared/types';
import { LoSelect } from '@/renderer/components/LoSelect';
import { ProjectDot } from '@/renderer/components/ProjectDot';
import { useEntryMutations, useRunning } from '@/renderer/lib/api';
import { clock } from '@/renderer/lib/format';
import { useNow } from '@/renderer/lib/useNow';
import { Icon } from '@/renderer/components/Icon';

/** Toggl-style start/stop: describe, pick a project, start. While running, edits go straight to the running entry. */
export function TimerBar({ projects }: { projects: Project[] }) {
  const running = useRunning().data ?? null;
  const m = useEntryMutations();
  const now = useNow(1000);
  const [description, setDescription] = useState('');
  const [projectId, setProjectId] = useState('');
  const [billable, setBillable] = useState(false);

  // Mirror the running entry (it may have been started from the menu bar).
  useEffect(() => {
    if (running) {
      setDescription(running.description);
      setProjectId(running.projectId ?? '');
      setBillable(running.billable);
    }
  }, [running?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  const project = projects.find((p) => p.id === projectId);
  const patchRunning = (patch: Parameters<typeof m.update.mutate>[0]['patch']) => running && m.update.mutate({ id: running.id, patch });

  const start = () => m.start.mutate({ description, projectId: projectId || null, billable });
  const stop = () =>
    m.stop.mutate(undefined, {
      onSuccess: () => {
        setDescription('');
        setProjectId('');
        setBillable(false);
      },
    });

  return (
    <form
      className="fo-timer"
      data-running={!!running}
      onSubmit={(e) => {
        e.preventDefault();
        if (running) patchRunning({ description });
        else start();
      }}
    >
      <input
        className="fo-timer-input"
        placeholder="What are you working on?"
        value={description}
        spellCheck={false}
        onChange={(e) => setDescription(e.target.value)}
        onBlur={() => running && description !== running.description && patchRunning({ description })}
      />
      <div className="fo-timer-project">
        <LoSelect
          value={projectId}
          leading={project && <ProjectDot color={project.color} />}
          aria-label="Project"
          placeholder="No project"
          onChange={(e) => {
            const v = e.target.value;
            setProjectId(v);
            const p = projects.find((x) => x.id === v);
            const b = p ? p.billableByDefault : billable;
            setBillable(b);
            if (running) patchRunning({ projectId: v || null, billable: b });
          }}
        >
          <option value="">No project</option>
          {projects
            .filter((p) => !p.archived)
            .map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
        </LoSelect>
      </div>
      <button
        type="button"
        className="fo-timer-bill"
        aria-pressed={billable}
        title={billable ? 'Billable' : 'Not billable'}
        onClick={() => {
          setBillable(!billable);
          if (running) patchRunning({ billable: !billable });
        }}
      >
        <Icon name="dollarsign.circle" aria-hidden="true" />
      </button>
      <span className="fo-timer-clock" aria-live="off">
        {running ? clock(now - running.start) : '0:00:00'}
      </span>
      {running ? (
        <button type="button" className="fo-timer-btn fo-timer-btn--stop" onClick={stop} aria-label="Stop timer" title="Stop timer">
          <Icon name="stop.fill" aria-hidden="true" />
        </button>
      ) : (
        <button type="submit" className="fo-timer-btn" aria-label="Start timer" title="Start timer">
          <Icon name="play.fill" aria-hidden="true" />
        </button>
      )}
    </form>
  );
}
