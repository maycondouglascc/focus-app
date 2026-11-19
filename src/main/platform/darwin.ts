import { execFile } from 'node:child_process';
import { powerMonitor, systemPreferences } from 'electron';
import type { PlatformMonitor, RawWindowSample } from './types';

const CHROMIUM = new Set([
  'com.google.Chrome',
  'com.google.Chrome.canary',
  'company.thebrowser.Browser',
  'com.brave.Browser',
  'com.microsoft.edgemac',
  'com.vivaldi.Vivaldi',
  'com.operasoftware.Opera',
]);
const SAFARI = new Set(['com.apple.Safari', 'com.apple.SafariTechnologyPreview']);

const script = (tabs: boolean) => `
function run() {
  var se = Application('System Events');
  var p = se.processes.whose({ frontmost: true })[0];
  var out = { app: '', bid: '', title: '', url: '', tab: '', priv: false, ax: true };
  try { out.app = p.displayedName(); } catch (e) { try { out.app = p.name(); } catch (e2) {} }
  try { out.bid = p.bundleIdentifier() || ''; } catch (e) {}
  try {
    var w = p.windows();
    if (w.length) out.title = w[0].name() || '';
  } catch (e) { if (String(e).indexOf('assistive') >= 0 || String(e).indexOf('-1719') >= 0 || String(e).indexOf('-25211') >= 0) out.ax = false; }
  ${
    tabs
      ? `try {
    var chromium = ${JSON.stringify([...CHROMIUM])};
    var safari = ${JSON.stringify([...SAFARI])};
    if (chromium.indexOf(out.bid) >= 0) {
      var b = Application(out.bid);
      if (b.windows.length) {
        var w0 = b.windows[0];
        try {
          var m = String(w0.mode() || '').toLowerCase();
          if (m === 'incognito' || m === 'inprivate' || m === 'private') out.priv = true;
        } catch (e) {}
        try {
          if (!out.priv && w0.incognito()) out.priv = true;
        } catch (e) {}
        if (!out.priv) {
          var t = w0.activeTab;
          out.url = t.url() || '';
          out.tab = t.title() || '';
        }
      }
    } else if (safari.indexOf(out.bid) >= 0) {
      var s = Application(out.bid);
      if (s.documents.length) { out.url = s.documents[0].url() || ''; out.tab = s.documents[0].name() || ''; }
    }
  } catch (e) {}`
      : ''
  }
  return JSON.stringify(out);
}`;

export const darwinMonitor: PlatformMonitor = {
  backend: 'macos-jxa',
  sessionType: 'darwin',
  capabilities: {
    windowTitles: true,
    browserTabs: true,
    idleDetection: true,
    mediaActivity: false,
  },

  sample(includeBrowserTabs: boolean): Promise<RawWindowSample | null> {
    return new Promise((resolve) => {
      execFile('/usr/bin/osascript', ['-l', 'JavaScript', '-e', script(includeBrowserTabs)], { timeout: 4000 }, (err, stdout) => {
        if (err) return resolve(null);
        try {
          const r = JSON.parse(String(stdout).trim()) as {
            app: string;
            bid: string;
            title: string;
            url: string;
            tab: string;
            priv?: boolean;
            ax: boolean;
          };
          if (!r.app) return resolve(null);
          resolve({
            app: r.app,
            bundleId: r.bid,
            title: r.title,
            url: r.url,
            tab: r.tab,
            isPrivate: Boolean(r.priv),
            titlesAllowed: r.ax,
          });
        } catch {
          resolve(null);
        }
      });
    });
  },

  getIdleSeconds(): Promise<number> {
    return Promise.resolve(powerMonitor.getSystemIdleTime());
  },

  getPermissionState() {
    const granted = systemPreferences.isTrustedAccessibilityClient(false);
    return Promise.resolve({ state: granted ? 'granted' : 'needs-permission' });
  },

  requestAccess(): Promise<boolean> {
    return Promise.resolve(systemPreferences.isTrustedAccessibilityClient(true));
  },
};
