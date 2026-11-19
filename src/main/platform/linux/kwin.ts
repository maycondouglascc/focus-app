import { resolveDesktopApp } from './desktop';
import { execText } from './exec';
import { getLinuxIdleSeconds, isLinuxFocusedMediaActive, startLinuxLockWatcher } from './idle';
import type { PlatformMonitor, RawWindowSample } from '../types';

export const kwinMonitor: PlatformMonitor = {
  backend: 'kwin',
  sessionType: 'wayland',
  capabilities: {
    windowTitles: true,
    browserTabs: false,
    idleDetection: true,
    mediaActivity: true,
  },

  async sample(): Promise<RawWindowSample | null> {
    const widOut = await execText('kdotool', ['getactivewindow'], 2000);
    const wid = widOut?.trim();
    if (!wid) return null;

    const [clsOut, titleOut, pidOut] = await Promise.all([
      execText('kdotool', ['getwindowclassname', wid], 2000),
      execText('kdotool', ['getwindowname', wid], 2000),
      execText('kdotool', ['getwindowpid', wid], 2000),
    ]);
    const cls = clsOut?.trim() || '';
    const title = titleOut?.trim() || '';
    const pid = pidOut ? Number(pidOut.trim()) || undefined : undefined;
    if (!cls && !title) return null;

    const info = resolveDesktopApp(cls, cls, cls);
    return {
      app: info.name,
      bundleId: cls || info.desktopId,
      title,
      url: '',
      tab: '',
      pid,
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
    const hasKdotool = await execText('which', ['kdotool'], 1500);
    if (!hasKdotool) {
      return {
        state: 'needs-setup',
        hint: 'Install kdotool (or run an X11 session) to record active window titles on KDE Wayland.',
      };
    }
    return { state: 'granted' };
  },

  async requestAccess() {
    const hasKdotool = await execText('which', ['kdotool'], 1500);
    return !!hasKdotool;
  },

  startLockWatcher: startLinuxLockWatcher,
};
