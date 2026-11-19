/**
 * Pure normalization and parsing helpers for Windows foreground window samples.
 * Kept free of Electron/Win32 runtime calls so every rule is unit-tested on any OS.
 */

export interface Win32RawPayload {
  hwnd: number;
  pid: number;
  exePath: string;
  fileDescription: string;
  productName: string;
  title: string;
  url: string;
}

const KNOWN_STEM_NAMES: Record<string, string> = {
  code: 'Visual Studio Code',
  'code - insiders': 'Visual Studio Code',
  cursor: 'Cursor',
  windsurf: 'Windsurf',
  zed: 'Zed',
  chrome: 'Google Chrome',
  msedge: 'Microsoft Edge',
  brave: 'Brave',
  firefox: 'Firefox',
  zen: 'Zen Browser',
  vivaldi: 'Vivaldi',
  opera: 'Opera',
  arc: 'Arc',
  windowsterminal: 'Windows Terminal',
  wt: 'Windows Terminal',
  explorer: 'File Explorer',
  powershell: 'PowerShell',
  pwsh: 'PowerShell',
  cmd: 'Command Prompt',
  notepad: 'Notepad',
  'notepad++': 'Notepad++',
  slack: 'Slack',
  discord: 'Discord',
  teams: 'Microsoft Teams',
  'ms-teams': 'Microsoft Teams',
  spotify: 'Spotify',
  figma: 'Figma',
  notion: 'Notion',
  obsidian: 'Obsidian',
  vlc: 'VLC',
  winword: 'Microsoft Word',
  excel: 'Microsoft Excel',
  powerpnt: 'Microsoft PowerPoint',
  outlook: 'Microsoft Outlook',
  olk: 'Microsoft Outlook',
};

/** `"C:\\Program Files\\Microsoft VS Code\\Code.exe"` -> `"Code"`. */
export function exeStem(exePath: string): string {
  const file = exePath.split(/[\\/]/).pop() || exePath;
  return file.replace(/\.exe$/i, '');
}

function isVersionDir(segment: string): boolean {
  return /^app-\d/i.test(segment);
}

/** Store package folder `"Name_1.2.3.0_x64__publisher"` -> `"pkg:name_publisher"`. */
function packageIdentity(lowerPath: string): string | null {
  const idx = lowerPath.indexOf('\\windowsapps\\');
  if (idx < 0) return null;
  const rest = lowerPath.slice(idx + '\\windowsapps\\'.length);
  const folder = rest.split('\\')[0];
  if (!folder) return null;
  const pubIdx = folder.indexOf('__');
  if (pubIdx < 0) return null;
  const left = folder.slice(0, pubIdx);
  const publisher = folder.slice(pubIdx + 2);
  const name = left.split('_')[0];
  if (!name || !publisher) return null;
  return `pkg:${name}_${publisher}`;
}

/**
 * Stable identity for a Windows executable path so Squirrel (`app-1.2.3\`) or
 * Microsoft Store version updates do not split activity history.
 */
export function normalizeExeKey(exePath: string): string {
  const normalized = exePath.trim().replace(/\//g, '\\').toLowerCase();
  if (!normalized) return '';
  const stem = exeStem(normalized);
  const pkg = packageIdentity(normalized);
  if (pkg) return `${pkg}/${stem}`;
  return normalized
    .split('\\')
    .filter((seg) => !isVersionDir(seg))
    .join('\\');
}

function isGenericName(name: string): boolean {
  const n = name.trim().toLowerCase();
  if (!n) return true;
  if (n === 'electron') return true;
  if (n.startsWith('java(tm)') || n.startsWith('java platform')) return true;
  if (n.includes('operating system') || n.includes('host process')) return true;
  return false;
}

/**
 * Picks a human-friendly app name from version metadata (FileDescription, then
 * ProductName) or falls back to a known display name / executable stem.
 */
export function chooseAppName(fileDescription: string | undefined, productName: string | undefined, exePath: string): string {
  const stem = exeStem(exePath);
  const stemLower = stem.toLowerCase();

  for (const candidate of [fileDescription, productName]) {
    const clean = (candidate || '').trim();
    if (clean && !isGenericName(clean)) return clean;
  }

  if (KNOWN_STEM_NAMES[stemLower]) return KNOWN_STEM_NAMES[stemLower];
  return stem;
}

const SKIPPED_SCHEMES = /^(?:about|chrome|chrome-extension|edge|brave|vivaldi|opera|moz-extension|file|javascript|data|blob|view-source):/i;

/**
 * Normalizes a raw address bar string read via UI Automation into a valid
 * http(s) URL, or returns `""` for search queries, empty values, or internal pages.
 */
export function normalizeBrowserUrl(raw: string): string {
  const trimmed = raw.trim();
  if (!trimmed || /\s/.test(trimmed)) return '';
  if (SKIPPED_SCHEMES.test(trimmed)) return '';

  let candidate = trimmed;
  if (!/^https?:\/\//i.test(candidate)) {
    // Reject unknown schemes like `mailto:...` or `ftp:...` while allowing `localhost:3000`.
    if (/^[a-z][a-z0-9+.-]*:/i.test(candidate) && !/^localhost:\d+/i.test(candidate)) {
      return '';
    }
    const isLocal = /^localhost(?::\d+)?(?:\/|$)/i.test(candidate) || /^127\.0\.0\.1(?::\d+)?(?:\/|$)/.test(candidate);
    if (!isLocal && !candidate.includes('.')) return '';
    candidate = `${isLocal ? 'http' : 'https'}://${candidate}`;
  }

  try {
    const parsed = new URL(candidate);
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return '';
    if (!parsed.hostname) return '';
    return parsed.href;
  } catch {
    return '';
  }
}

export interface WorkerMessage {
  ready?: boolean;
  uia?: boolean;
  audio?: boolean;
  id?: number;
  ok?: boolean;
  data?: unknown;
  error?: string;
}

export function parseWorkerLine(line: string): WorkerMessage | null {
  const trimmed = line.trim();
  if (!trimmed || !trimmed.startsWith('{')) return null;
  try {
    return JSON.parse(trimmed) as WorkerMessage;
  } catch {
    return null;
  }
}
