import { QueryClient, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import type { FocusApi, TimerStartInput } from '@/shared/ipc';
import type { EntryInput, EntryPatch, ProjectInput, ProjectPatch, Range, Settings } from '@/shared/types';
import { createDemoApi } from './demoApi';

export const isNative = !!window.focusApp;
const demo = window.focusApp && window.focusDemo ? { ...createDemoApi(), apps: window.focusApp.apps } : null; // demo data, real app icons
export const api: FocusApi = demo ?? window.focusApp ?? createDemoApi();

export const queryClient = new QueryClient({
  defaultOptions: { queries: { staleTime: 30_000, refetchOnWindowFocus: true, retry: 1 } },
});

/** Keeps every screen current when the tray, the monitor or the Claude reader change data behind the renderer. */
export function useLiveData() {
  const qc = useQueryClient();
  useEffect(
    () =>
      api.onChanged((scope) => {
        void qc.invalidateQueries({ queryKey: [scope] });
        if (scope === 'entries') void qc.invalidateQueries({ queryKey: ['running'] });
      }),
    [qc],
  );
}

export const useProjects = () => useQuery({ queryKey: ['projects'], queryFn: api.projects.list });
export const useEntries = (r: Range) => useQuery({ queryKey: ['entries', r.from, r.to], queryFn: () => api.entries.list(r) });
export const useActivity = (r: Range, enabled = true) =>
  useQuery({ queryKey: ['activity', r.from, r.to], queryFn: () => api.activity.list(r), enabled, refetchInterval: 30_000 });
export const useRunning = () => useQuery({ queryKey: ['running'], queryFn: api.entries.running });
export const useSettings = () => useQuery({ queryKey: ['settings'], queryFn: api.settings.get });
export const useMonitorStatus = () => useQuery({ queryKey: ['monitor'], queryFn: api.monitor.status, refetchInterval: 10_000 });

function useInvalidate() {
  const qc = useQueryClient();
  return (...keys: string[]) => keys.forEach((k) => void qc.invalidateQueries({ queryKey: [k] }));
}

export function useEntryMutations() {
  const inv = useInvalidate();
  const done = () => inv('entries', 'running');
  return {
    create: useMutation({ mutationFn: (i: EntryInput) => api.entries.create(i), onSuccess: done }),
    update: useMutation({ mutationFn: ({ id, patch }: { id: string; patch: EntryPatch }) => api.entries.update(id, patch), onSuccess: done }),
    remove: useMutation({ mutationFn: (id: string) => api.entries.remove(id), onSuccess: done }),
    start: useMutation({ mutationFn: (i: TimerStartInput) => api.timer.start(i), onSuccess: done }),
    stop: useMutation({ mutationFn: () => api.timer.stop(), onSuccess: done }),
  };
}

export function useProjectMutations() {
  const inv = useInvalidate();
  const done = () => inv('projects', 'entries');
  return {
    create: useMutation({ mutationFn: (i: ProjectInput) => api.projects.create(i), onSuccess: done }),
    update: useMutation({ mutationFn: ({ id, patch }: { id: string; patch: ProjectPatch }) => api.projects.update(id, patch), onSuccess: done }),
    remove: useMutation({ mutationFn: (id: string) => api.projects.remove(id), onSuccess: done }),
  };
}

export function useSaveSettings() {
  const inv = useInvalidate();
  return useMutation({ mutationFn: (p: Partial<Settings>) => api.settings.save(p), onSuccess: () => inv('settings', 'monitor', 'activity') });
}

export function useDeleteActivity() {
  const inv = useInvalidate();
  return useMutation({ mutationFn: (ids: number[]) => api.activity.remove(ids), onSuccess: () => inv('activity') });
}

/** Icons for a set of app names. Cached for the session: icons don't change while the app runs. */
export function useAppIcons(names: string[]) {
  const key = [...new Set(names)].sort();
  return useQuery({ queryKey: ['appIcons', key], queryFn: () => api.apps.icons(key), staleTime: Infinity, enabled: key.length > 0 }).data ?? {};
}
