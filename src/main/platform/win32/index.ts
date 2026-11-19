import { powerMonitor } from 'electron';
import type { PlatformMonitor, RawWindowSample } from '../types';
import { lookupWin32ExePath, rememberWin32ExePath } from './icons';
import { chooseAppName, normalizeBrowserUrl, normalizeExeKey } from './normalize';
import { Win32Worker } from './worker';

export function createWin32Monitor(): PlatformMonitor {
  const worker = new Win32Worker();

  return {
    backend: 'win32',
    sessionType: 'win32',
    capabilities: {
      windowTitles: true,
      browserTabs: true,
      idleDetection: true,
      mediaActivity: true,
    },

    async sample(includeBrowserTabs: boolean): Promise<RawWindowSample | null> {
      const raw = await worker.sample(includeBrowserTabs);
      if (!raw) return null;

      const appName = chooseAppName(raw.fileDescription, raw.productName, raw.exePath);
      if (!appName) return null;

      const bundleId = normalizeExeKey(raw.exePath);
      rememberWin32ExePath(bundleId, appName, raw.exePath);

      return {
        app: appName,
        bundleId,
        title: raw.title,
        url: includeBrowserTabs ? normalizeBrowserUrl(raw.url) : '',
        tab: '',
        pid: raw.pid || undefined,
        titlesAllowed: true,
      };
    },

    getIdleSeconds(): Promise<number> {
      return Promise.resolve(powerMonitor.getSystemIdleTime());
    },

    async isFocusedMediaActive(sample: RawWindowSample): Promise<boolean> {
      const exePath = lookupWin32ExePath(sample.bundleId, sample.app);
      return worker.isProcessPlayingAudio(sample.pid ?? 0, exePath);
    },

    getPermissionState() {
      return Promise.resolve({ state: 'granted' as const });
    },

    requestAccess(): Promise<boolean> {
      return Promise.resolve(true);
    },

    dispose() {
      worker.dispose();
    },
  };
}
