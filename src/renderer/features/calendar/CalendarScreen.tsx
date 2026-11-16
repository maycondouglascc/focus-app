import { useCallback, useMemo, useState } from 'react';
import type { TimeEntry } from '@/shared/types';
import { LoButton } from '@/renderer/components/LoButton';
import { useActivity, useEntries, useProjects, useSettings } from '@/renderer/lib/api';
import { addDays, isoWeek, startOfWeek } from '@/renderer/lib/time';
import { hm, money, rangeLabel } from '@/renderer/lib/format';
import { earningsByCurrency, entryMs, unionMs } from '@/renderer/lib/stats';
import { Segmented } from '@/renderer/components/Segmented';
import { useNow } from '@/renderer/lib/useNow';
import { TimerBar } from './TimerBar';
import { WeekGrid, type CalendarMode } from './WeekGrid';
import { EntryEditor, type Draft } from './EntryEditor';
import { ActivityPopover } from './ActivityPopover';
import { usePresence } from '@/renderer/lib/usePresence';
import type { ActivitySession } from './layout';

const LS_KEY = 'focus.calendarMode';
const MODES: CalendarMode[] = ['mix', 'time', 'activity'];

export function CalendarScreen() {
  const now = useNow(30_000);
  const [weekStart, setWeekStart] = useState(() => startOfWeek(Date.now()));
  const range = useMemo(() => ({ from: weekStart, to: addDays(weekStart, 7) }), [weekStart]);
  const [mode, setModeState] = useState<CalendarMode>(() => {
    try {
      const v = localStorage.getItem(LS_KEY) as CalendarMode | null;
      return v && MODES.includes(v) ? v : 'mix';
    } catch {
      return 'mix';
    }
  });
  const showActivity = mode !== 'time';
  const projects = useProjects().data ?? [];
  const settings = useSettings().data;
  const entries = useEntries(range).data ?? [];
  const activity = useActivity(range, showActivity).data ?? [];

  const [editing, setEditing] = useState<{ draft: Draft; anchor: DOMRect | null } | null>(null);
  const [actBlock, setActBlock] = useState<{ session: ActivitySession; anchor: DOMRect } | null>(null);
  const closeEditor = useCallback(() => setEditing(null), []);
  const closeAct = useCallback(() => setActBlock(null), []);
  // Popovers stay a moment after closing to play their exit.
  const [editorShown, editorClosing] = usePresence(editing);
  const [actShown, actClosing] = usePresence(actBlock);

  const byId = new Map(projects.map((p) => [p.id, p]));
  const total = entries.reduce((s, e) => s + entryMs(e, range.from, range.to, now), 0);
  const earned = earningsByCurrency(entries, byId, range.from, range.to);
  const thisWeek = weekStart === startOfWeek(now);

  // While a new entry is being described, keep its block on the grid.
  const withDraft: TimeEntry[] =
    editing && !editing.draft.id ? [...entries, { ...editing.draft, id: '__draft' }] : entries;

  const setMode = (m: CalendarMode) => {
    setModeState(m);
    if (m === 'activity') setEditing(null);
    try {
      localStorage.setItem(LS_KEY, m);
    } catch {
      /* per-viewer convenience only */
    }
  };
  const active = unionMs(activity.filter((a) => a.kind === 'app'), range.from, range.to);

  return (
    <div className="fo-screen">
      <div className="fo-top">
        <TimerBar projects={projects} />
      </div>

      <div className="fo-toolbar">
        <div className="fo-weeknav">
          <LoButton variant="ghost" icon="chevron.left" aria-label="Previous week" onClick={() => setWeekStart(addDays(weekStart, -7))} />
          <LoButton small onClick={() => setWeekStart(startOfWeek(Date.now()))} disabled={thisWeek}>
            Today
          </LoButton>
          <LoButton variant="ghost" icon="chevron.right" aria-label="Next week" onClick={() => setWeekStart(addDays(weekStart, 7))} />
          <h1 className="fo-weektitle">
            {thisWeek ? 'This week' : rangeLabel(range.from, range.to)}
            <small>W{isoWeek(weekStart)}</small>
          </h1>
        </div>
        <div className="fo-weekstats">
          {mode !== 'activity' && (
            <>
              <span>
                <b>{hm(total)}</b> tracked
              </span>
              {/* Billable hours live in Reports; the week header keeps one number per question, each with its word. */}
              {earned.map(([cur, v]) => (
                <span key={cur}>
                  <b>{money(v, cur)}</b> earned
                </span>
              ))}
            </>
          )}
          {mode !== 'time' && (
            <span>
              <b>{hm(active)}</b> active
            </span>
          )}
        </div>
        <div className="fo-toolbar-actions">
          <Segmented
            label="Calendar shows"
            value={mode}
            onChange={setMode}
            options={[
              { value: 'mix', label: 'Mix', ariaLabel: 'Time entries and activity' },
              { value: 'time', label: 'Time', ariaLabel: 'Time entries only' },
              { value: 'activity', label: 'Activity', ariaLabel: 'Activity only' },
            ]}
          />
        </div>
      </div>

      <WeekGrid
        weekStart={weekStart}
        entries={withDraft}
        activity={activity}
        projects={projects}
        mode={mode}
        now={now}
        scrollToHour={settings?.dayStartHour ?? 8}
        onOpenEntry={(draft, anchor) => {
          if (draft.id === '__draft') return;
          setActBlock(null);
          if (draft.id && editing?.draft.id === draft.id) return setEditing(null);
          setEditing({ draft, anchor });
        }}
        onOpenActivity={(session, anchor) => {
          setEditing(null);
          setActBlock(actBlock?.session.key === session.key ? null : { session, anchor });
        }}
      />

      {editorShown && (
        <EntryEditor
          key={`${editorShown.draft.id ?? 'new'}-${editorShown.draft.start}`}
          draft={editorShown.draft}
          anchor={editorShown.anchor}
          projects={projects}
          onClose={closeEditor}
          closing={editorClosing}
        />
      )}
      {actShown && (
        <ActivityPopover
          session={actShown.session}
          anchor={actShown.anchor}
          onClose={closeAct}
          closing={actClosing}
          onLog={(b, description) => {
            setActBlock(null);
            setEditing({
              draft: { projectId: null, description, start: b.start, end: b.end, billable: false, source: 'activity' },
              anchor: actShown.anchor,
            });
          }}
        />
      )}
    </div>
  );
}
