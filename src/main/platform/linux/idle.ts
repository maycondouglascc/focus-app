import { spawn, type ChildProcess } from 'node:child_process';
import { powerMonitor } from 'electron';
import { resolveDesktopApp } from './desktop';
import { execText, parseGVariantTuple } from './exec';
import type { RawWindowSample } from '../types';

async function getMutterIdleSeconds(): Promise<number | null> {
  const out = await execText(
    'gdbus',
    ['call', '--session', '--dest', 'org.gnome.Mutter.IdleMonitor', '--object-path', '/org/gnome/Mutter/IdleMonitor/Core', '--method', 'org.gnome.Mutter.IdleMonitor.GetIdletime'],
    1500,
  );
  if (!out) return null;
  const parsed = parseGVariantTuple(out);
  const ms = parsed?.[0];
  return typeof ms === 'number' && Number.isFinite(ms) ? Math.floor(ms / 1000) : null;
}

async function getScreenSaverIdleSeconds(): Promise<number | null> {
  const out = await execText(
    'gdbus',
    ['call', '--session', '--dest', 'org.freedesktop.ScreenSaver', '--object-path', '/org/freedesktop/ScreenSaver', '--method', 'org.freedesktop.ScreenSaver.GetSessionIdleTime'],
    1500,
  );
  if (!out) return null;
  const parsed = parseGVariantTuple(out);
  const sec = parsed?.[0];
  return typeof sec === 'number' && Number.isFinite(sec) ? sec : null;
}

export async function getLinuxIdleSeconds(preferGnome: boolean): Promise<number> {
  if (preferGnome) {
    const mutter = await getMutterIdleSeconds();
    if (mutter !== null) return mutter;
  }
  const ss = await getScreenSaverIdleSeconds();
  if (ss !== null) return ss;
  if (!preferGnome) {
    const mutter = await getMutterIdleSeconds();
    if (mutter !== null) return mutter;
  }
  return powerMonitor.getSystemIdleTime();
}

const GNOME_IDLE_OR_SUSPEND_INHIBIT_FLAGS = 12; // GSM_INHIBITOR_FLAG_SUSPEND (4) | GSM_INHIBITOR_FLAG_IDLE (8)

function normalizeToken(raw: string): string {
  return raw.trim().toLowerCase().replace(/\.desktop$/i, '').replace(/[^a-z0-9]+/g, '');
}

function collectAppKeys(raw: string): string[] {
  const clean = raw.trim().replace(/\.desktop$/i, '');
  if (!clean) return [];
  const keys = new Set<string>();
  const full = normalizeToken(clean);
  if (full) keys.add(full);

  const dotParts = clean.split('.');
  if (dotParts.length >= 2) {
    const tail = normalizeToken(dotParts[dotParts.length - 1]);
    if (tail) keys.add(tail);
  }
  const underscoreParts = clean.split('_');
  if (underscoreParts.length === 2) {
    const head = normalizeToken(underscoreParts[0]);
    if (head) keys.add(head);
  }
  return [...keys];
}

function matchesSampleApp(
  sample: RawWindowSample,
  candidate: { pid?: number; appId?: string; appName?: string; binary?: string },
): boolean {
  if (candidate.pid && candidate.pid > 0 && sample.pid && sample.pid > 0 && candidate.pid === sample.pid) {
    return true;
  }
  const sampleKeys = new Set<string>([...collectAppKeys(sample.app), ...collectAppKeys(sample.bundleId)]);
  if (sampleKeys.size === 0) return false;

  const candKeys = new Set<string>();
  for (const raw of [candidate.appId, candidate.appName, candidate.binary]) {
    if (!raw) continue;
    for (const k of collectAppKeys(raw)) candKeys.add(k);
  }
  if (candidate.appId || candidate.binary) {
    const resolved = resolveDesktopApp(candidate.appId ?? '', '', candidate.binary ?? '');
    for (const k of [...collectAppKeys(resolved.desktopId), ...collectAppKeys(resolved.name)]) {
      candKeys.add(k);
    }
  }
  for (const k of candKeys) {
    if (sampleKeys.has(k)) return true;
  }
  return false;
}

async function hasGnomeMediaInhibitor(sample: RawWindowSample): Promise<boolean> {
  const inhibited = await execText(
    'gdbus',
    [
      'call',
      '--session',
      '--dest',
      'org.gnome.SessionManager',
      '--object-path',
      '/org/gnome/SessionManager',
      '--method',
      'org.gnome.SessionManager.IsInhibited',
      String(GNOME_IDLE_OR_SUSPEND_INHIBIT_FLAGS),
    ],
    1000,
  );
  if (inhibited?.trim() !== '(true,)') return false;

  const listOut = await execText(
    'gdbus',
    [
      'call',
      '--session',
      '--dest',
      'org.gnome.SessionManager',
      '--object-path',
      '/org/gnome/SessionManager',
      '--method',
      'org.gnome.SessionManager.GetInhibitors',
    ],
    1000,
  );
  if (!listOut) return false;

  const paths = listOut.match(/\/org\/gnome\/SessionManager\/Inhibitor\d+/g) ?? [];
  if (paths.length === 0) return false;

  const hits = await Promise.all(
    paths.map(async (objectPath) => {
      const [appOut, flagsOut] = await Promise.all([
        execText(
          'gdbus',
          ['call', '--session', '--dest', 'org.gnome.SessionManager', '--object-path', objectPath, '--method', 'org.gnome.SessionManager.Inhibitor.GetAppId'],
          1000,
        ),
        execText(
          'gdbus',
          ['call', '--session', '--dest', 'org.gnome.SessionManager', '--object-path', objectPath, '--method', 'org.gnome.SessionManager.Inhibitor.GetFlags'],
          1000,
        ),
      ]);
      const appId = appOut ? parseGVariantTuple(appOut)?.[0] : undefined;
      const flags = flagsOut ? parseGVariantTuple(flagsOut)?.[0] : undefined;
      if (typeof appId !== 'string' || typeof flags !== 'number') return false;
      if ((flags & GNOME_IDLE_OR_SUSPEND_INHIBIT_FLAGS) === 0) return false;
      return matchesSampleApp(sample, { appId });
    }),
  );
  return hits.some(Boolean);
}

async function hasActiveAudioStream(sample: RawWindowSample): Promise<boolean> {
  const out = await execText('pactl', ['list', 'sink-inputs'], 1500, { ...process.env, LC_ALL: 'C' });
  if (!out) return false;

  const blocks = out.split(/^Sink Input #\d+/m);
  for (const block of blocks) {
    if (!/\bCorked:\s*no\b/i.test(block)) continue;
    const pidMatch = block.match(/^\s*application\.process\.id\s*=\s*"([^"]*)"/m);
    const nameMatch = block.match(/^\s*application\.name\s*=\s*"([^"]*)"/m);
    const binMatch = block.match(/^\s*application\.process\.binary\s*=\s*"([^"]*)"/m);
    const pid = pidMatch ? Number(pidMatch[1]) : undefined;
    if (
      matchesSampleApp(sample, {
        pid: Number.isFinite(pid) ? pid : undefined,
        appName: nameMatch?.[1],
        binary: binMatch?.[1],
      })
    ) {
      return true;
    }
  }
  return false;
}

export async function isLinuxFocusedMediaActive(sample: RawWindowSample, preferGnome: boolean): Promise<boolean> {
  if (preferGnome && (await hasGnomeMediaInhibitor(sample))) return true;
  return hasActiveAudioStream(sample);
}

function spawnMonitor(args: string[], onLine: (line: string) => void): ChildProcess | null {
  try {
    const child = spawn('gdbus', args, { stdio: ['ignore', 'pipe', 'ignore'] });
    let buf = '';
    child.stdout?.setEncoding('utf8');
    child.stdout?.on('data', (chunk: string) => {
      buf += chunk;
      const lines = buf.split('\n');
      buf = lines.pop() ?? '';
      for (const line of lines) onLine(line);
    });
    child.on('error', () => undefined);
    return child;
  } catch {
    return null;
  }
}

export function startLinuxLockWatcher(onLock: () => void, onUnlock?: () => void): () => void {
  const children: ChildProcess[] = [];

  const handleScreenSaverLine = (line: string) => {
    if (!line.includes('ActiveChanged')) return;
    if (line.includes('(true')) onLock();
    else if (line.includes('(false')) onUnlock?.();
  };

  const handleLoginLine = (line: string) => {
    if (line.includes('.Session.Lock')) onLock();
    else if (line.includes('.Session.Unlock')) onUnlock?.();
    else if (line.includes("'LockedHint': <true>")) onLock();
    else if (line.includes("'LockedHint': <false>")) onUnlock?.();
  };

  for (const args of [
    ['monitor', '--session', '--dest', 'org.gnome.ScreenSaver'],
    ['monitor', '--session', '--dest', 'org.freedesktop.ScreenSaver'],
  ]) {
    const p = spawnMonitor(args, handleScreenSaverLine);
    if (p) children.push(p);
  }
  const loginProc = spawnMonitor(['monitor', '--system', '--dest', 'org.freedesktop.login1'], handleLoginLine);
  if (loginProc) children.push(loginProc);

  return () => {
    for (const c of children) {
      try {
        c.kill();
      } catch {
        /* process already exited */
      }
    }
  };
}
