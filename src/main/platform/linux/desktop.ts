import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export interface DesktopEntry {
  id: string;
  name: string;
  icon: string;
  wmClass: string;
  execBase: string;
}

export interface DesktopAppInfo {
  name: string;
  desktopId: string;
  iconName: string;
}

interface DesktopIndex {
  builtAt: number;
  byId: Map<string, DesktopEntry>;
  byWmClass: Map<string, DesktopEntry>;
  bySuffix: Map<string, DesktopEntry>;
  byExec: Map<string, DesktopEntry>;
  byName: Map<string, DesktopEntry>;
}

const INDEX_TTL_MS = 5 * 60_000;
let cachedIndex: DesktopIndex | null = null;

function applicationDirs(): string[] {
  const home = os.homedir();
  const dataHome = process.env.XDG_DATA_HOME || path.join(home, '.local', 'share');
  const dataDirs = (process.env.XDG_DATA_DIRS || '/usr/local/share:/usr/share').split(':').filter(Boolean);
  const dirs = [
    path.join(dataHome, 'applications'),
    path.join(home, '.local', 'share', 'flatpak', 'exports', 'share', 'applications'),
    '/var/lib/flatpak/exports/share/applications',
    '/var/lib/snapd/desktop/applications',
    ...dataDirs.map((d) => path.join(d, 'applications')),
  ];
  return [...new Set(dirs)];
}

function parseDesktopFile(filePath: string, desktopId: string): DesktopEntry | null {
  let content: string;
  try {
    content = fs.readFileSync(filePath, 'utf8');
  } catch {
    return null;
  }
  let inEntry = false;
  let name = '';
  let icon = '';
  let wmClass = '';
  let exec = '';

  for (const rawLine of content.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    if (line.startsWith('[')) {
      if (inEntry) break;
      inEntry = line === '[Desktop Entry]';
      continue;
    }
    if (!inEntry) continue;
    const eq = line.indexOf('=');
    if (eq <= 0) continue;
    const key = line.slice(0, eq).trim();
    const val = line.slice(eq + 1).trim();
    if (key === 'Name' && !name) name = val;
    else if (key === 'Icon' && !icon) icon = val;
    else if (key === 'StartupWMClass' && !wmClass) wmClass = val;
    else if (key === 'Exec' && !exec) exec = val;
  }
  if (!name) return null;

  // Strip quotes/env wrappers from Exec= to extract the executable basename (e.g. `env FOO=1 /usr/bin/code %F` -> `code`).
  let execBase = '';
  if (exec) {
    const tokens = exec.match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g) ?? [];
    for (const rawTok of tokens) {
      const tok = rawTok.replace(/^["']|["']$/g, '');
      if (tok === 'env' || /^[A-Za-z_][A-Za-z0-9_]*=/.test(tok) || tok.startsWith('%') || tok.startsWith('-')) continue;
      execBase = path.basename(tok).toLowerCase();
      break;
    }
  }

  return { id: desktopId, name, icon, wmClass, execBase };
}

function collectDesktopFiles(dir: string, prefix = '', depth = 0): { filePath: string; id: string }[] {
  if (depth > 2) return [];
  let entries: fs.Dirent[];
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return [];
  }
  const out: { filePath: string; id: string }[] = [];
  for (const ent of entries) {
    const full = path.join(dir, ent.name);
    if (ent.isDirectory()) {
      out.push(...collectDesktopFiles(full, prefix ? `${prefix}-${ent.name}` : ent.name, depth + 1));
    } else if ((ent.isFile() || ent.isSymbolicLink()) && ent.name.endsWith('.desktop')) {
      const stem = ent.name.slice(0, -'.desktop'.length);
      out.push({ filePath: full, id: prefix ? `${prefix}-${stem}` : stem });
    }
  }
  return out;
}

function addSuffixKeys(map: Map<string, DesktopEntry>, id: string, entry: DesktopEntry) {
  const lower = id.toLowerCase();
  const dotParts = lower.split('.');
  if (dotParts.length >= 2) {
    const tail = dotParts[dotParts.length - 1];
    if (tail && !map.has(tail)) map.set(tail, entry);
  }
  // Snap desktop IDs look like `firefox_firefox` or `code_code`.
  const underscoreParts = lower.split('_');
  if (underscoreParts.length === 2 && underscoreParts[0] && !map.has(underscoreParts[0])) {
    map.set(underscoreParts[0], entry);
  }
}

function buildIndex(): DesktopIndex {
  const idx: DesktopIndex = {
    builtAt: Date.now(),
    byId: new Map(),
    byWmClass: new Map(),
    bySuffix: new Map(),
    byExec: new Map(),
    byName: new Map(),
  };

  for (const dir of applicationDirs()) {
    for (const { filePath, id } of collectDesktopFiles(dir)) {
      const lowerId = id.toLowerCase();
      if (idx.byId.has(lowerId)) continue;
      const entry = parseDesktopFile(filePath, id);
      if (!entry) continue;
      idx.byId.set(lowerId, entry);
      if (entry.wmClass) {
        const key = entry.wmClass.toLowerCase();
        if (!idx.byWmClass.has(key)) idx.byWmClass.set(key, entry);
      }
      addSuffixKeys(idx.bySuffix, id, entry);
      if (entry.execBase && !idx.byExec.has(entry.execBase)) idx.byExec.set(entry.execBase, entry);
      const lowerName = entry.name.toLowerCase();
      if (!idx.byName.has(lowerName)) idx.byName.set(lowerName, entry);
    }
  }
  return idx;
}

function getIndex(allowRefresh = false): DesktopIndex {
  if (!cachedIndex || (allowRefresh && Date.now() - cachedIndex.builtAt > INDEX_TTL_MS)) {
    cachedIndex = buildIndex();
  }
  return cachedIndex;
}

function lookupEntry(idx: DesktopIndex, candidate: string): DesktopEntry | undefined {
  const clean = candidate.trim().replace(/\.desktop$/i, '');
  if (!clean) return undefined;
  const lower = clean.toLowerCase();
  const direct = idx.byId.get(lower) ?? idx.byWmClass.get(lower) ?? idx.bySuffix.get(lower) ?? idx.byExec.get(lower) ?? idx.byName.get(lower);
  if (direct) return direct;

  const dotParts = lower.split('.');
  if (dotParts.length >= 2) {
    const tail = dotParts[dotParts.length - 1];
    return idx.byId.get(tail) ?? idx.byWmClass.get(tail) ?? idx.bySuffix.get(tail) ?? idx.byExec.get(tail);
  }
  return undefined;
}

export function prettifyAppId(raw: string): string {
  const clean = raw.trim().replace(/\.desktop$/i, '');
  if (!clean) return '';
  const parts = clean.split('.');
  const base = parts.length >= 3 ? parts[parts.length - 1] : clean;
  if (/^[a-z0-9-]+$/.test(base)) {
    return base.charAt(0).toUpperCase() + base.slice(1);
  }
  return base;
}

export function resolveDesktopApp(appId: string, wmClass = '', fallbackName = ''): DesktopAppInfo {
  const candidates = [appId, wmClass, fallbackName].filter(Boolean);
  let idx = getIndex(false);
  for (const c of candidates) {
    const hit = lookupEntry(idx, c);
    if (hit) return { name: hit.name, desktopId: hit.id, iconName: hit.icon };
  }
  // If an app was installed after startup, refresh the index once TTL expires.
  idx = getIndex(true);
  for (const c of candidates) {
    const hit = lookupEntry(idx, c);
    if (hit) return { name: hit.name, desktopId: hit.id, iconName: hit.icon };
  }
  const raw = appId || wmClass || fallbackName;
  return {
    name: fallbackName || prettifyAppId(raw) || raw,
    desktopId: raw.replace(/\.desktop$/i, ''),
    iconName: '',
  };
}
