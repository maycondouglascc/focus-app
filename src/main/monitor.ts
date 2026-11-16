import { execFile } from 'node:child_process';
import { app, powerMonitor, systemPreferences } from 'electron';
import { activity, settings } from './db';
import type { MonitorStatus } from '../shared/types';

/**
 * Samples the frontmost app + window every few seconds and writes contiguous runs as activity rows.
 * One JXA call per sample: System Events gives the app and its front window title (needs Accessibility);
 * Chromium browsers and Safari give the active tab's URL and title (needs Automation, asked once per browser).
 * Nothing leaves the machine.
 */

const POLL_MS = 5000;
/** A gap longer than this between samples (sleep, a hung call) closes the current run instead of bridging it. */
const MAX_BRIDGE_MS = 30_000;

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
  var out = { app: '', bid: '', title: '', url: '', tab: '', ax: true };
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
      if (b.windows.length) { var t = b.windows[0].activeTab; out.url = t.url() || ''; out.tab = t.title() || ''; }
    } else if (safari.indexOf(out.bid) >= 0) {
      var s = Application(out.bid);
      if (s.documents.length) { out.url = s.documents[0].url() || ''; out.tab = s.documents[0].name() || ''; }
    }
  } catch (e) {}`
      : ''
  }
  return JSON.stringify(out);
}`;

interface Sample {
  app: string;
  bundleId: string;
  detail: string;
  context: string;
  url: string;
}

function sample(tabs: boolean): Promise<(Sample & { ax: boolean }) | null> {
  return new Promise((resolve) => {
    execFile('/usr/bin/osascript', ['-l', 'JavaScript', '-e', script(tabs)], { timeout: 4000 }, (err, stdout) => {
      if (err) return resolve(null);
      try {
        const r = JSON.parse(String(stdout).trim()) as { app: string; bid: string; title: string; url: string; tab: string; ax: boolean };
        if (!r.app) return resolve(null);
        resolve({ ...describe(r.app, r.bid, r.title, r.url, r.tab), ax: r.ax });
      } catch {
        resolve(null);
      }
    });
  });
}

/** Turns a raw window title into the part worth reading: the file, the tab, the project. */
export function describe(rawApp: string, bundleId: string, title: string, url: string, tab: string): Sample {
  let app = rawApp;
  // Apps run from source (`electron .`) all report as "Electron"; their window title is the real app name.
  if (rawApp === 'Electron' && title.trim()) {
    const parts = title.trim().split(/\s[—–-]\s/);
    app = parts[parts.length - 1].slice(0, 60);
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
  // Drop the "— App Name" suffix most Mac apps append to their window titles.
  const suffix = new RegExp(`\\s[—–-]\\s${app.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`, 'i');
  detail = detail.replace(suffix, '');
  // Editors: "file.ts — project" → detail "file.ts", context "project".
  if (/^(Code|Cursor|Visual Studio Code|Windsurf|Zed)$/i.test(app)) {
    const parts = detail.split(/\s[—–]\s/);
    if (parts.length >= 2) {
      context = parts[parts.length - 1];
      detail = parts.slice(0, -1).join(' — ');
    }
  }
  if (detail === app) detail = '';
  return { app, bundleId, detail: detail.slice(0, 300), context: context.slice(0, 120), url: url.slice(0, 1000) };
}

// ── Run tracking ─────────────────────────────────────────────────────────

let timer: NodeJS.Timeout | undefined;
let current: { id: number; key: string; app: string; detail: string; start: number; last: number } | null = null;
let idle = false;
let accessibility = true;
let busy = false;
let onChange: () => void = () => undefined;

/** This app, whether packaged ("Focus") or run from source (reported as Electron, named by its window title). */
export const isSelf = (s: { app: string }) => s.app === app.getName() || s.app === 'Focus';

const keyOf = (s: Sample) => `${s.app}\u0000${s.detail}\u0000${s.context}`;

function close(at: number) {
  if (!current) return;
  activity.setEnd(current.id, Math.max(current.start + 1000, at));
  current = null;
  onChange();
}

async function tick() {
  if (busy) return;
  const cfg = settings.get();
  if (!cfg.monitorEnabled) return close(Date.now());
  const now = Date.now();
  const idleSec = powerMonitor.getSystemIdleTime();
  if (idleSec >= cfg.idleMinutes * 60) {
    if (!idle) {
      idle = true;
      close(now - idleSec * 1000);
    }
    return;
  }
  idle = false;
  busy = true;
  try {
    const s = await sample(cfg.browserTabs);
    if (!s) return;
    accessibility = s.ax;
    // Focus never tracks itself: time spent in this window is a break in the stream, like being idle.
    if (isSelf(s)) return close(Date.now());
    const at = Date.now();
    const key = keyOf(s);
    if (current && current.key === key && at - current.last <= MAX_BRIDGE_MS) {
      current.last = at;
      activity.setEnd(current.id, at);
      return;
    }
    // A new run starts where the previous one ended when they touch, so the stream has no gaps.
    const start = current && at - current.last <= MAX_BRIDGE_MS ? current.last : at;
    close(start);
    const id = activity.insert({ kind: 'app', ...s, start, end: at + 1000 });
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
  powerMonitor.on('lock-screen', () => close(Date.now()));
  powerMonitor.on('suspend', () => close(Date.now()));
}

export function stopMonitor() {
  if (timer) clearInterval(timer);
  timer = undefined;
  close(Date.now());
}

export function monitorStatus(): MonitorStatus {
  const ax = process.platform === 'darwin' ? systemPreferences.isTrustedAccessibilityClient(false) : true;
  return {
    enabled: settings.get().monitorEnabled,
    accessibility: ax && accessibility,
    idle,
    current: current ? { app: current.app, detail: current.detail, since: current.start } : null,
    claudeLastScan: null,
    native: true,
  };
}

/** Shows the macOS prompt that leads to Privacy & Security → Accessibility. Returns the current grant. */
export function requestAccessibility(): boolean {
  if (process.platform !== 'darwin') return true;
  return systemPreferences.isTrustedAccessibilityClient(true);
}
