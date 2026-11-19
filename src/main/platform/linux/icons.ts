import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { resolveDesktopApp } from './desktop';
import { execText } from './exec';

let themeChainPromise: Promise<string[]> | null = null;

function iconBaseDirs(): string[] {
  const home = os.homedir();
  const dataHome = process.env.XDG_DATA_HOME || path.join(home, '.local', 'share');
  const dataDirs = (process.env.XDG_DATA_DIRS || '/usr/local/share:/usr/share').split(':').filter(Boolean);
  return [
    ...new Set([
      path.join(home, '.icons'),
      path.join(dataHome, 'icons'),
      path.join(home, '.local', 'share', 'flatpak', 'exports', 'share', 'icons'),
      '/var/lib/flatpak/exports/share/icons',
      '/var/lib/snapd/desktop/icons',
      ...dataDirs.map((d) => path.join(d, 'icons')),
    ]),
  ];
}

function readThemeInherits(themeName: string, baseDirs: string[]): string[] {
  for (const base of baseDirs) {
    const indexFile = path.join(base, themeName, 'index.theme');
    try {
      const content = fs.readFileSync(indexFile, 'utf8');
      const m = content.match(/^Inherits=(.+)$/m);
      if (m) {
        return m[1]
          .split(',')
          .map((s) => s.trim())
          .filter(Boolean);
      }
    } catch {
      /* theme index not present in this dir */
    }
  }
  return [];
}

async function detectPrimaryTheme(): Promise<string> {
  const gsettings = await execText('gsettings', ['get', 'org.gnome.desktop.interface', 'icon-theme'], 1500);
  if (gsettings) {
    const cleaned = gsettings.trim().replace(/^['"]|['"]$/g, '');
    if (cleaned) return cleaned;
  }
  const gtkPaths = [
    path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), '.config'), 'gtk-3.0', 'settings.ini'),
    path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), '.config'), 'gtk-4.0', 'settings.ini'),
  ];
  for (const p of gtkPaths) {
    try {
      const ini = fs.readFileSync(p, 'utf8');
      const m = ini.match(/^gtk-icon-theme-name\s*=\s*(.+)$/m);
      if (m) return m[1].trim().replace(/^['"]|['"]$/g, '');
    } catch {
      /* ignore missing gtk config */
    }
  }
  return 'Yaru';
}

async function getThemeChain(): Promise<string[]> {
  if (!themeChainPromise) {
    themeChainPromise = (async () => {
      const baseDirs = iconBaseDirs();
      const seeds = [await detectPrimaryTheme(), 'Yaru', 'Adwaita', 'breeze', 'hicolor'];
      const ordered: string[] = [];
      const seen = new Set<string>();
      const queue = [...seeds];
      while (queue.length) {
        const name = queue.shift()!;
        if (!name || seen.has(name)) continue;
        seen.add(name);
        ordered.push(name);
        for (const parent of readThemeInherits(name, baseDirs)) {
          if (!seen.has(parent)) queue.push(parent);
        }
      }
      if (!seen.has('hicolor')) ordered.push('hicolor');
      return ordered;
    })();
  }
  return themeChainPromise;
}

const PNG_SUBDIRS = [
  '64x64/apps',
  '48x48@2x/apps',
  '128x128/apps',
  '48x48/apps',
  '256x256/apps',
  '256x256@2x/apps',
  '32x32@2x/apps',
  '32x32/apps',
  '512x512/apps',
  '24x24@2x/apps',
  '24x24/apps',
  '22x22@2x/apps',
  '22x22/apps',
  '16x16@2x/apps',
  '16x16/apps',
  'apps/64',
  'apps/48',
  'apps/128',
  'apps/32',
];

const SVG_SUBDIRS = ['scalable/apps', 'apps/scalable', 'scalable/devices', 'scalable/places', 'scalable/mimetypes', 'scalable'];

function encodeIconFile(filePath: string): string | null {
  try {
    const ext = path.extname(filePath).toLowerCase();
    if (ext === '.svg' || ext === '.svgz') {
      if (ext === '.svgz') return null;
      const buf = fs.readFileSync(filePath);
      return `data:image/svg+xml;base64,${buf.toString('base64')}`;
    }
    if (ext === '.png') {
      const buf = fs.readFileSync(filePath);
      return `data:image/png;base64,${buf.toString('base64')}`;
    }
  } catch {
    return null;
  }
  return null;
}

function tryFile(filePath: string): string | null {
  try {
    if (fs.statSync(filePath).isFile()) return encodeIconFile(filePath);
  } catch {
    /* file does not exist */
  }
  return null;
}

async function resolveIconName(iconName: string): Promise<string | null> {
  if (!iconName) return null;
  if (path.isAbsolute(iconName)) {
    const direct = tryFile(iconName);
    if (direct) return direct;
  }

  const clean = iconName.replace(/\.(png|svg|xpm)$/i, '');
  const themes = await getThemeChain();
  const baseDirs = iconBaseDirs();

  for (const theme of themes) {
    for (const base of baseDirs) {
      const themeRoot = path.join(base, theme);
      if (!fs.existsSync(themeRoot)) continue;
      for (const sub of SVG_SUBDIRS) {
        const hit = tryFile(path.join(themeRoot, sub, `${clean}.svg`));
        if (hit) return hit;
      }
      for (const sub of PNG_SUBDIRS) {
        const hit = tryFile(path.join(themeRoot, sub, `${clean}.png`));
        if (hit) return hit;
      }
    }
  }

  for (const ext of ['.svg', '.png']) {
    const hit = tryFile(path.join('/usr/share/pixmaps', `${clean}${ext}`));
    if (hit) return hit;
  }
  return null;
}

export async function loadLinuxIcons(bundleIds: string[]): Promise<Record<string, string>> {
  if (!bundleIds.length) return {};
  const out: Record<string, string> = {};
  await Promise.all(
    bundleIds.map(async (id) => {
      const desktop = resolveDesktopApp(id, id, id);
      const candidates = [desktop.iconName, desktop.desktopId, id, id.toLowerCase()].filter(Boolean);
      for (const candidate of [...new Set(candidates)]) {
        const url = await resolveIconName(candidate);
        if (url) {
          out[id] = url;
          return;
        }
      }
    }),
  );
  return out;
}
