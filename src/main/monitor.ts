import { app, powerMonitor } from 'electron';
import { APP_TITLE_ALIASES, isPrivateBrowserWindow } from './browserPrivacy';
import { activity, settings } from './db';
import { darwinMonitor } from './platform/darwin';
import { createLinuxMonitor } from './platform/linux';
import { createWin32Monitor } from './platform/win32';
import type { PlatformMonitor } from './platform/types';
import type { MonitorStatus } from '../shared/types';

/**
 * Samples the frontmost app + window every few seconds and writes contiguous runs as activity rows.
 * Delegates OS-specific window sampling, idle time, permissions and lock watchers to a platform provider.
 * Nothing leaves the machine.
 */

const POLL_MS = 5000;
/** A gap longer than this between samples (sleep, a hung call) closes the current run instead of bridging it. */
const MAX_BRIDGE_MS = 30_000;

interface Sample {
  app: string;
  bundleId: string;
  detail: string;
  context: string;
  url: string;
  pid?: number;
}

let provider: PlatformMonitor | null = null;
function getProvider(): PlatformMonitor {
  if (!provider) {
    provider =
      process.platform === 'darwin'
        ? darwinMonitor
        : process.platform === 'win32'
          ? createWin32Monitor()
          : createLinuxMonitor();
  }
  return provider;
}

/** Turns a raw window title into the part worth reading: the file, the tab, the project. */
export function describe(rawApp: string, bundleId: string, title: string, url: string, tab: string, pid?: number): Sample {
  let appName = rawApp;
  // Apps run from source (`electron .`) can report as "Electron"; their window title is the real app name.
  if (/^electron$/i.test(rawApp) && title.trim()) {
    const parts = title.trim().split(/\s[—–-]\s/);
    appName = parts[parts.length - 1].slice(0, 60);
    title = parts.slice(0, -1).join(' — ');
  }
  let detail = (tab || title || '').trim();
  let context = '';
  if (url) {
    try {
      context = new URL(url).hostname.replace(/^www\./, '');
    } catch {
      /* not a URL: leave the host empty */
    }
  }
  // Drop the "— App Name" suffix most apps append to their window titles.
  const suffixNames = [appName, ...(APP_TITLE_ALIASES[appName.toLowerCase()] ?? [])];
  for (const name of suffixNames) {
    const suffix = new RegExp(`\\s[—–-]\\s${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`, 'i');
    if (suffix.test(detail)) {
      detail = detail.replace(suffix, '');
      break;
    }
  }
  // Editors: "file.ts — project" (or "file.ts - project" on Windows) → detail "file.ts", context "project".
  if (/^(Code|Cursor|Visual Studio Code|Windsurf|Zed)$/i.test(appName)) {
    const parts = detail.split(/\s[—–-]\s/);
    if (parts.length >= 2) {
      context = parts[parts.length - 1];
      detail = parts.slice(0, -1).join(' — ');
    }
  }
  if (suffixNames.some((n) => n.toLowerCase() === detail.toLowerCase())) detail = '';
  return { app: appName, bundleId, detail: detail.slice(0, 300), context: context.slice(0, 120), url: url.slice(0, 1000), pid };
}

// ── Run tracking ─────────────────────────────────────────────────────────

const MEDIA_CHECK_AFTER_IDLE_SEC = 15;

let timer: NodeJS.Timeout | undefined;
let current: { id: number; key: string; app: string; detail: string; start: number; last: number } | null = null;
let idle = false;
let locked = false;
let lastMediaActiveAt = 0;
let accessibility = true;
let busy = false;
let onChange: () => void = () => undefined;
let stopLockWatcher: (() => void) | null = null;

const onSystemLock = () => {
  locked = true;
  lastMediaActiveAt = 0;
  close(Date.now());
};

const onSystemUnlock = () => {
  locked = false;
};

/** This app, whether packaged ("Focus") or run from source (Electron / PID match). */
export const isSelf = (s: { app: string; bundleId?: string; pid?: number }) => {
  if (s.pid !== undefined && s.pid > 0 && s.pid === process.pid) return true;
  if (s.app === app.getName() || s.app === 'Focus') return true;
  const bid = (s.bundleId || '').toLowerCase().replace(/\.desktop$/, '');
  return bid === 'focus' || bid === 'focus-app' || bid.endsWith('\\focus.exe') || bid.endsWith('/focus.exe');
};

const keyOf = (s: Sample) => `${s.app}\u0000${s.detail}\u0000${s.context}`;

function close(at: number) {
  if (!current) return;
  activity.setEnd(current.id, Math.max(current.start + 1000, at));
  current = null;
  onChange();
}

/** Clears the active run pointer if its row was deleted while still open. */
export function forgetDeletedActivity(ids: number[]) {
  if (current && ids.includes(current.id)) current = null;
}

async function tick() {
  if (busy) return;
  const cfg = settings.get();
  if (!cfg.monitorEnabled || !cfg.onboarded || locked) return close(Date.now());
  const p = getProvider();
  const now = Date.now();
  const idleSec = await p.getIdleSeconds();
  if (idleSec < MEDIA_CHECK_AFTER_IDLE_SEC) lastMediaActiveAt = 0;

  const thresholdSec = cfg.idleMinutes * 60;
  const canCheckMedia = cfg.keepActiveOnMedia && !!p.isFocusedMediaActive;

  if (idleSec >= thresholdSec && !canCheckMedia) {
    if (!idle) {
      idle = true;
      close(now - idleSec * 1000);
    }
    return;
  }

  busy = true;
  try {
    const raw = await p.sample(cfg.browserTabs && p.capabilities.browserTabs);
    const at = Date.now();
    const s = raw ? describe(raw.app, raw.bundleId, raw.title, raw.url, raw.tab, raw.pid) : null;
    const isPrivateBrowser = !!raw && isPrivateBrowserWindow(raw);

    if (canCheckMedia && raw && s && !isSelf(s) && !isPrivateBrowser && idleSec >= MEDIA_CHECK_AFTER_IDLE_SEC) {
      if (await p.isFocusedMediaActive!(raw)) lastMediaActiveAt = at;
    }

    const effectiveIdleSec =
      lastMediaActiveAt > 0
        ? Math.min(idleSec, Math.max(0, Math.floor((at - lastMediaActiveAt) / 1000)))
        : idleSec;

    if (effectiveIdleSec >= thresholdSec) {
      if (!idle) {
        idle = true;
        close(at - effectiveIdleSec * 1000);
      }
      return;
    }

    idle = false;
    if (!raw || !s) return;
    accessibility = raw.titlesAllowed;
    // Focus never tracks itself or private browser windows: time spent there is a break in the stream, like being idle.
    if (isSelf(s) || isPrivateBrowser) return close(at);
    const key = keyOf(s);
    if (current && current.key === key && at - current.last <= MAX_BRIDGE_MS) {
      current.last = at;
      activity.setEnd(current.id, at);
      return;
    }
    // A new run starts where the previous one ended when they touch, so the stream has no gaps.
    const start = current && at - current.last <= MAX_BRIDGE_MS ? current.last : at;
    close(start);
    const id = activity.insert({
      kind: 'app',
      app: s.app,
      bundleId: s.bundleId,
      detail: s.detail,
      context: s.context,
      url: s.url,
      start,
      end: at + 1000,
    });
    current = { id, key, app: s.app, detail: s.detail, start, last: at };
    onChange();
  } finally {
    busy = false;
  }
}

export function startMonitor(changed: () => void) {
  onChange = changed;
  stopMonitor();
  timer = setInterval(() => void tick(), POLL_MS);
  void tick();
  powerMonitor.on('lock-screen', onSystemLock);
  powerMonitor.on('suspend', onSystemLock);
  powerMonitor.on('unlock-screen', onSystemUnlock);
  powerMonitor.on('resume', onSystemUnlock);
  const p = getProvider();
  stopLockWatcher = p.startLockWatcher ? p.startLockWatcher(onSystemLock, onSystemUnlock) : null;
}

export function stopMonitor() {
  if (timer) clearInterval(timer);
  timer = undefined;
  powerMonitor.removeListener('lock-screen', onSystemLock);
  powerMonitor.removeListener('suspend', onSystemLock);
  powerMonitor.removeListener('unlock-screen', onSystemUnlock);
  powerMonitor.removeListener('resume', onSystemUnlock);
  stopLockWatcher?.();
  stopLockWatcher = null;
  provider?.dispose?.();
  locked = false;
  lastMediaActiveAt = 0;
  close(Date.now());
}

export async function monitorStatus(): Promise<MonitorStatus> {
  const p = getProvider();
  const perm = await p.getPermissionState();
  const permGranted = perm.state === 'granted';
  return {
    enabled: settings.get().monitorEnabled,
    accessibility: permGranted && (process.platform !== 'darwin' || accessibility),
    idle,
    current: current ? { app: current.app, detail: current.detail, since: current.start } : null,
    claudeLastScan: null,
    native: true,
    platform: process.platform === 'darwin' || process.platform === 'linux' || process.platform === 'win32' ? process.platform : 'linux',
    backend: p.backend,
    sessionType: p.sessionType,
    permissionState: perm.state,
    setupHint: perm.hint,
    capabilities: p.capabilities,
  };
}

/** Triggers platform-appropriate permission / extension setup and returns whether access is active. */
export async function requestAccessibility(): Promise<boolean> {
  return getProvider().requestAccess();
}
