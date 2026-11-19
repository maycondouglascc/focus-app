import { resolveDesktopApp } from './desktop';
import { execText } from './exec';
import { getLinuxIdleSeconds, isLinuxFocusedMediaActive, startLinuxLockWatcher } from './idle';
import type { PlatformMonitor, RawWindowSample } from '../types';

interface HyprWindow {
  class?: string;
  initialClass?: string;
  title?: string;
  pid?: number;
}

export const hyprlandMonitor: PlatformMonitor = {
  backend: 'hyprland',
  sessionType: 'wayland',
  capabilities: {
    windowTitles: true,
    browserTabs: false,
    idleDetection: true,
    mediaActivity: true,
  },

  async sample(): Promise<RawWindowSample | null> {
    const out = await execText('hyprctl', ['activewindow', '-j'], 2000);
    if (!out) return null;
    let win: HyprWindow;
    try {
      win = JSON.parse(out) as HyprWindow;
    } catch {
      return null;
    }
    const cls = win.class || win.initialClass || '';
    const title = win.title || '';
    if (!cls && !title) return null;

    const info = resolveDesktopApp(cls, win.initialClass || '', cls);
    return {
      app: info.name,
      bundleId: cls || info.desktopId,
      title,
      url: '',
      tab: '',
      pid: win.pid,
      titlesAllowed: true,
    };
  },

  getIdleSeconds() {
    return getLinuxIdleSeconds(false);
  },

  isFocusedMediaActive(sample) {
    return isLinuxFocusedMediaActive(sample, false);
  },

  async getPermissionState() {
    const ok = await execText('hyprctl', ['version'], 1500);
    return ok ? { state: 'granted' } : { state: 'unavailable', hint: 'Could not reach hyprctl.' };
  },

  async requestAccess() {
    const ok = await execText('hyprctl', ['version'], 1500);
    return !!ok;
  },

  startLockWatcher: startLinuxLockWatcher,
};
