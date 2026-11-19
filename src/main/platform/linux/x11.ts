import { resolveDesktopApp } from './desktop';
import { execText } from './exec';
import { getLinuxIdleSeconds, isLinuxFocusedMediaActive, startLinuxLockWatcher } from './idle';
import type { PlatformMonitor, RawWindowSample } from '../types';

function unescapeXpropString(raw: string): string {
  return raw.replace(/\\(["\\])/g, '$1');
}

function parseActiveWindowId(stdout: string): string | null {
  const m = stdout.match(/window id #\s*(0x[0-9a-fA-F]+)/);
  if (!m) return null;
  const id = m[1];
  if (/^0x0+$/i.test(id)) return null;
  return id;
}

function parseWindowProps(stdout: string): { title: string; wmInstance: string; wmClass: string; pid?: number } {
  let utf8Title = '';
  let legacyTitle = '';
  let wmInstance = '';
  let wmClass = '';
  let pid: number | undefined;

  for (const line of stdout.split('\n')) {
    if (line.startsWith('_NET_WM_NAME(')) {
      const m = line.match(/=\s*"((?:[^"\\]|\\.)*)"\s*$/);
      if (m) utf8Title = unescapeXpropString(m[1]);
    } else if (line.startsWith('WM_NAME(')) {
      const m = line.match(/=\s*"((?:[^"\\]|\\.)*)"\s*$/);
      if (m) legacyTitle = unescapeXpropString(m[1]);
    } else if (line.startsWith('WM_CLASS(')) {
      const matches = [...line.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((x) => unescapeXpropString(x[1]));
      if (matches[0]) wmInstance = matches[0];
      if (matches[1]) wmClass = matches[1];
    } else if (line.startsWith('_NET_WM_PID(')) {
      const m = line.match(/=\s*(\d+)/);
      if (m) pid = Number(m[1]);
    }
  }
  return { title: utf8Title || legacyTitle, wmInstance, wmClass, pid };
}

export const x11Monitor: PlatformMonitor = {
  backend: 'x11',
  sessionType: 'x11',
  capabilities: {
    windowTitles: true,
    browserTabs: false,
    idleDetection: true,
    mediaActivity: true,
  },

  async sample(): Promise<RawWindowSample | null> {
    const rootOut = await execText('xprop', ['-root', '_NET_ACTIVE_WINDOW'], 2000);
    if (!rootOut) return null;
    const wid = parseActiveWindowId(rootOut);
    if (!wid) return null;

    const propsOut = await execText('xprop', ['-id', wid, '_NET_WM_NAME', 'WM_NAME', 'WM_CLASS', '_NET_WM_PID'], 2000);
    if (!propsOut) return null;
    const { title, wmInstance, wmClass, pid } = parseWindowProps(propsOut);
    const primaryClass = wmClass || wmInstance;
    if (!primaryClass && !title) return null;

    const info = resolveDesktopApp(primaryClass, wmInstance, primaryClass);
    return {
      app: info.name,
      bundleId: primaryClass || info.desktopId,
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
    const hasXprop = await execText('which', ['xprop'], 1500);
    if (!hasXprop) {
      return { state: 'needs-setup', hint: 'Install x11-utils (xprop) to record active windows on X11.' };
    }
    return { state: 'granted' };
  },

  async requestAccess() {
    const hasXprop = await execText('which', ['xprop'], 1500);
    return !!hasXprop;
  },

  startLockWatcher: startLinuxLockWatcher,
};
