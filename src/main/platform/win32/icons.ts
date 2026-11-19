import fs from 'node:fs';
import path from 'node:path';
import { app } from 'electron';

const exePathByKey = new Map<string, string>();
const exePathByName = new Map<string, string>();

export function rememberWin32ExePath(bundleId: string, appName: string, exePath: string) {
  if (!exePath) return;
  if (bundleId) exePathByKey.set(bundleId.toLowerCase(), exePath);
  if (appName) exePathByName.set(appName.toLowerCase(), exePath);
}

export function lookupWin32ExePath(bundleId?: string, appName?: string): string {
  if (bundleId) {
    const hit = exePathByKey.get(bundleId.toLowerCase());
    if (hit) return hit;
  }
  if (appName) {
    const hit = exePathByName.get(appName.toLowerCase());
    if (hit) return hit;
  }
  return '';
}

/**
 * Resolves a normalized bundleId (which may have Squirrel `app-x.y.z\` stripped)
 * back to an existing `.exe` on disk so Electron can extract its icon.
 */
function resolveExecutablePath(bundleId: string, appName: string): string | null {
  const remembered = lookupWin32ExePath(bundleId, appName);
  if (remembered && fs.existsSync(remembered)) return remembered;

  const candidate = bundleId.trim();
  if (!candidate || candidate.startsWith('pkg:')) return null;
  if (fs.existsSync(candidate)) return candidate;

  // Squirrel-installed apps (e.g. Discord, Slack, Postman) live under `<dir>\app-<ver>\<exe>`.
  const dir = path.win32.dirname(candidate);
  const base = path.win32.basename(candidate);
  if (!dir || !base || !fs.existsSync(dir)) return null;
  try {
    const entries = fs.readdirSync(dir, { withFileTypes: true });
    const appDirs = entries
      .filter((e) => e.isDirectory() && /^app-\d/i.test(e.name))
      .map((e) => e.name)
      .sort()
      .reverse();
    for (const sub of appDirs) {
      const full = path.win32.join(dir, sub, base);
      if (fs.existsSync(full)) return full;
    }
  } catch {
    /* ignore unreadable directory */
  }
  return null;
}

/**
 * Extracts PNG data URLs for Windows executables via Electron's native `app.getFileIcon`.
 */
export async function loadWin32Icons(items: Array<{ name: string; bundleId: string }>): Promise<Record<string, string>> {
  if (!items.length) return {};
  const out: Record<string, string> = {};
  await Promise.all(
    items.map(async ({ name, bundleId }) => {
      const exePath = resolveExecutablePath(bundleId, name);
      if (!exePath) return;
      try {
        const icon = await app.getFileIcon(exePath, { size: 'large' });
        if (!icon.isEmpty()) out[name] = icon.toDataURL();
      } catch {
        /* ignore inaccessible icon */
      }
    }),
  );
  return out;
}
