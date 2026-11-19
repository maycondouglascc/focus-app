import type { MonitorBackendId, MonitorCapabilities, PermissionState } from '../../shared/types';

export type { MonitorBackendId, MonitorCapabilities, PermissionState };

export interface RawWindowSample {
  /** Human-friendly app name (e.g. "Visual Studio Code", "Firefox"). */
  app: string;
  /** Platform identifier: macOS bundleId, Windows normalized exe key, Linux app_id / desktop ID / WM_CLASS. */
  bundleId: string;
  title: string;
  url: string;
  tab: string;
  pid?: number;
  /** True when the platform backend directly confirms a private/incognito window. */
  isPrivate?: boolean;
  /** True when window titles are accessible; false if permission/extension is missing. */
  titlesAllowed: boolean;
}

export interface PlatformMonitor {
  readonly backend: MonitorBackendId;
  readonly sessionType: 'darwin' | 'win32' | 'wayland' | 'x11' | 'unknown';
  readonly capabilities: MonitorCapabilities;
  sample(includeBrowserTabs: boolean): Promise<RawWindowSample | null>;
  getIdleSeconds(): Promise<number>;
  isFocusedMediaActive?(sample: RawWindowSample): Promise<boolean>;
  getPermissionState(): Promise<{ state: PermissionState; hint?: string }>;
  requestAccess(): Promise<boolean>;
  startLockWatcher?(onLock: () => void, onUnlock?: () => void): () => void;
  dispose?(): void;
}
