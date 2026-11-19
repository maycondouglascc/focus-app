import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { app } from 'electron';
import { resolveDesktopApp } from './desktop';
import { execText, parseGVariantTuple } from './exec';
import { getLinuxIdleSeconds, isLinuxFocusedMediaActive, startLinuxLockWatcher } from './idle';
import type { PlatformMonitor, RawWindowSample } from '../types';

const EXT_UUID = 'focus-tracker@focus.app';
const BUS_NAME = 'org.focus.WindowTracker';
const OBJECT_PATH = '/org/focus/WindowTracker';
const METHOD = 'org.focus.WindowTracker.GetFocusedWindow';

function extensionSourceDir(): string {
  return app.isPackaged
    ? path.join(process.resourcesPath, 'gnome-extension', EXT_UUID)
    : path.join(app.getAppPath(), 'resources', 'gnome-extension', EXT_UUID);
}

function extensionDestDir(): string {
  return path.join(os.homedir(), '.local', 'share', 'gnome-shell', 'extensions', EXT_UUID);
}

async function isDbusActive(): Promise<boolean> {
  const out = await execText(
    'gdbus',
    ['call', '--session', '--dest', 'org.freedesktop.DBus', '--object-path', '/org/freedesktop/DBus', '--method', 'org.freedesktop.DBus.NameHasOwner', BUS_NAME],
    1500,
  );
  return !!out && out.trim() === '(true,)';
}

async function getEnabledExtensionsList(): Promise<string[]> {
  const raw = await execText('gsettings', ['get', 'org.gnome.shell', 'enabled-extensions'], 1500);
  if (!raw) return [];
  const matches = [...raw.matchAll(/'([^']+)'/g)].map((m) => m[1]);
  return matches;
}

async function ensureExtensionEnabledInGsettings(): Promise<void> {
  const userDisabled = await execText('gsettings', ['get', 'org.gnome.shell', 'disable-user-extensions'], 1500);
  if (userDisabled?.trim() === 'true') {
    await execText('gsettings', ['set', 'org.gnome.shell', 'disable-user-extensions', 'false'], 1500);
  }
  const current = await getEnabledExtensionsList();
  if (!current.includes(EXT_UUID)) {
    const next = [...current, EXT_UUID];
    const formatted = `[${next.map((id) => `'${id.replace(/'/g, "\\'")}'`).join(', ')}]`;
    await execText('gsettings', ['set', 'org.gnome.shell', 'enabled-extensions', formatted], 1500);
  }
}

export const gnomeWaylandMonitor: PlatformMonitor = {
  backend: 'gnome-wayland',
  sessionType: 'wayland',
  capabilities: {
    windowTitles: true,
    browserTabs: false,
    idleDetection: true,
    mediaActivity: true,
  },

  async sample(): Promise<RawWindowSample | null> {
    const out = await execText('gdbus', ['call', '--session', '--dest', BUS_NAME, '--object-path', OBJECT_PATH, '--method', METHOD], 2000);
    if (!out) return null;
    const parsed = parseGVariantTuple(out);
    if (!parsed || parsed.length < 4) return null;

    const rawAppId = typeof parsed[0] === 'string' ? parsed[0] : '';
    const wmClass = typeof parsed[1] === 'string' ? parsed[1] : '';
    const title = typeof parsed[2] === 'string' ? parsed[2] : '';
    const pid = typeof parsed[3] === 'number' && parsed[3] > 0 ? parsed[3] : undefined;

    // GNOME Shell assigns synthetic `window:<n>` IDs to unmatched windows; fall back to WM_CLASS for those.
    const cleanAppId = rawAppId.startsWith('window:') ? '' : rawAppId.replace(/\.desktop$/i, '');
    if (!cleanAppId && !wmClass && !title) return null;

    const info = resolveDesktopApp(cleanAppId, wmClass, wmClass || cleanAppId);
    return {
      app: info.name,
      bundleId: cleanAppId || wmClass || info.desktopId,
      title,
      url: '',
      tab: '',
      pid,
      titlesAllowed: true,
    };
  },

  getIdleSeconds() {
    return getLinuxIdleSeconds(true);
  },

  isFocusedMediaActive(sample) {
    return isLinuxFocusedMediaActive(sample, true);
  },

  async getPermissionState() {
    if (await isDbusActive()) return { state: 'granted' };

    const installedOnDisk = fs.existsSync(path.join(extensionDestDir(), 'extension.js'));
    if (!installedOnDisk) {
      return {
        state: 'needs-setup',
        hint: 'Install the Focus GNOME extension so Focus can read the active window and title on Wayland.',
      };
    }

    const userDisabled = await execText('gsettings', ['get', 'org.gnome.shell', 'disable-user-extensions'], 1500);
    if (userDisabled?.trim() === 'true') {
      return {
        state: 'needs-setup',
        hint: 'GNOME user extensions are disabled. Click Enable to allow user extensions and activate Focus.',
      };
    }

    const infoOut = await execText('gnome-extensions', ['info', EXT_UUID], 1500);
    const enabledInGsettings = (await getEnabledExtensionsList()).includes(EXT_UUID);
    const isEnabled = enabledInGsettings || (!!infoOut && /State:\s*(ENABLED|ACTIVE|INITIALIZED)|Enabled:\s*Yes/i.test(infoOut));

    if (isEnabled) {
      return {
        state: 'needs-relogin',
        hint: 'Extension installed. Log out and back in once so GNOME Shell loads it.',
      };
    }

    return {
      state: 'needs-setup',
      hint: 'Enable the Focus GNOME extension so Focus can read active window titles.',
    };
  },

  async requestAccess() {
    const src = extensionSourceDir();
    const dest = extensionDestDir();
    try {
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      fs.cpSync(src, dest, { recursive: true });
    } catch (err) {
      console.error('[gnome] failed to copy extension', err);
      return false;
    }

    await execText('gnome-extensions', ['enable', EXT_UUID], 2000);
    await ensureExtensionEnabledInGsettings();
    await new Promise((r) => setTimeout(r, 250));
    return isDbusActive();
  },

  startLockWatcher: startLinuxLockWatcher,
};
