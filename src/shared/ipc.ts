import type {
  Activity,
  EntryInput,
  EntryPatch,
  MonitorStatus,
  Project,
  ProjectInput,
  ProjectPatch,
  Range,
  Settings,
  TimeEntry,
} from './types';

export const Channels = {
  projectList: 'project:list',
  projectCreate: 'project:create',
  projectUpdate: 'project:update',
  projectDelete: 'project:delete',
  entryList: 'entry:list',
  entryCreate: 'entry:create',
  entryUpdate: 'entry:update',
  entryDelete: 'entry:delete',
  entryRunning: 'entry:running',
  timerStart: 'timer:start',
  timerStop: 'timer:stop',
  activityList: 'activity:list',
  activityDelete: 'activity:delete',
  activityClear: 'activity:clear',
  appIcons: 'apps:icons',
  monitorStatus: 'monitor:status',
  monitorRequestAccess: 'monitor:request-access',
  settingsGet: 'settings:get',
  settingsSave: 'settings:save',
  /** main → renderer: entries / activity changed outside the renderer (tray, monitor). */
  changed: 'data:changed',
} as const;

export type ChangeScope = 'entries' | 'activity' | 'projects';

export interface TimerStartInput {
  projectId: string | null;
  description: string;
  billable: boolean;
}

export interface FocusApi {
  projects: {
    list: () => Promise<Project[]>;
    create: (input: ProjectInput) => Promise<Project>;
    update: (id: string, patch: ProjectPatch) => Promise<Project>;
    remove: (id: string) => Promise<void>;
  };
  entries: {
    list: (range: Range) => Promise<TimeEntry[]>;
    create: (input: EntryInput) => Promise<TimeEntry>;
    update: (id: string, patch: EntryPatch) => Promise<TimeEntry>;
    remove: (id: string) => Promise<void>;
    running: () => Promise<TimeEntry | null>;
  };
  timer: {
    start: (input: TimerStartInput) => Promise<TimeEntry>;
    stop: () => Promise<TimeEntry | null>;
  };
  activity: {
    list: (range: Range) => Promise<Activity[]>;
    remove: (ids: number[]) => Promise<void>;
    clear: () => Promise<void>;
  };
  apps: {
    /** App name → PNG data URL of its icon, or null when there is none worth showing. */
    icons: (names: string[]) => Promise<Record<string, string | null>>;
  };
  monitor: {
    status: () => Promise<MonitorStatus>;
    requestAccess: () => Promise<boolean>;
  };
  settings: {
    get: () => Promise<Settings>;
    save: (patch: Partial<Settings>) => Promise<Settings>;
  };
  onChanged: (cb: (scope: ChangeScope) => void) => () => void;
}
