// Shared between the main process and the renderer. All times are epoch milliseconds.

export interface Project {
  id: string;
  name: string;
  client: string;
  color: string;
  /** Hourly rate in `currency`. 0 = internal / not billed. */
  rate: number;
  currency: string;
  /** New entries on this project start as billable. */
  billableByDefault: boolean;
  archived: boolean;
  createdAt: number;
}
export type ProjectInput = Omit<Project, 'id' | 'createdAt' | 'archived'> & { archived?: boolean };
export type ProjectPatch = Partial<ProjectInput>;

export type EntrySource = 'manual' | 'calendar' | 'timer' | 'activity';

export interface TimeEntry {
  id: string;
  projectId: string | null;
  description: string;
  start: number;
  /** null while the timer is running. */
  end: number | null;
  billable: boolean;
  source: EntrySource;
}
export type EntryInput = Omit<TimeEntry, 'id'>;
export type EntryPatch = Partial<EntryInput>;

/** `app`: the frontmost window, sampled. `claude`: a Claude Code session, read from ~/.claude/projects. */
export type ActivityKind = 'app' | 'claude';

export interface Activity {
  id: number;
  kind: ActivityKind;
  /** App name ("Figma", "Google Chrome") or "Claude Code". */
  app: string;
  bundleId: string;
  /** What inside the app: window title / file / tab title / session title. */
  detail: string;
  /** Secondary context: web host for browsers, project folder for Claude Code. */
  context: string;
  url: string;
  start: number;
  end: number;
}

export interface Settings {
  currency: string;
  monitorEnabled: boolean;
  /** Stop counting after this many minutes without keyboard or mouse input. */
  idleMinutes: number;
  /** Keep counting when the focused app is playing audio/video or inhibiting idle. */
  keepActiveOnMedia: boolean;
  browserTabs: boolean;
  claudeSessions: boolean;
  /** Hour the week view scrolls to when it opens. */
  dayStartHour: number;
  /** Set when the first-run welcome is finished. Nothing is recorded before that. */
  onboarded: boolean;
}

export type MonitorBackendId = 'macos-jxa' | 'win32' | 'gnome-wayland' | 'sway' | 'hyprland' | 'kwin' | 'x11' | 'none';

export type PermissionState = 'granted' | 'needs-permission' | 'needs-setup' | 'needs-relogin' | 'unavailable';

export interface MonitorCapabilities {
  windowTitles: boolean;
  browserTabs: boolean;
  idleDetection: boolean;
  mediaActivity: boolean;
}

export interface MonitorStatus {
  enabled: boolean;
  /** True when Focus can read window titles (macOS Accessibility granted, Windows, or active Linux backend). */
  accessibility: boolean;
  idle: boolean;
  current: { app: string; detail: string; since: number } | null;
  claudeLastScan: number | null;
  /** Available inside the desktop app (macOS, Windows, or Linux). */
  native: boolean;
  platform?: 'darwin' | 'linux' | 'win32';
  backend?: MonitorBackendId;
  sessionType?: 'darwin' | 'win32' | 'wayland' | 'x11' | 'unknown';
  permissionState?: PermissionState;
  setupHint?: string;
  capabilities?: MonitorCapabilities;
}

export interface Range {
  from: number;
  to: number;
}
